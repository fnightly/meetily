// audio/onnx_model.rs
//
// Generic ONNX inference, for models that are not the Parakeet transcriber.
//
// The Parakeet engine has its own loader tuned to its three-stage
// preprocessor/encoder/decoder graphs and its vocabulary. The models needed for
// speaker diarisation — a Pyannote-style segmentation graph and a speaker
// embedding graph — are plain "feed named float tensors, read named tensors back"
// graphs with no vocabulary and no decoder loop, so they need a smaller, generic
// wrapper rather than a second copy of the Parakeet machinery.
//
// Runtime: inference goes through `ort`, which is already loaded dynamically from
// the bundled `onnxruntime.dll`. Nothing here introduces a second ONNX Runtime —
// see `audio/speaker_cluster.rs` for why that matters.

use std::path::{Path, PathBuf};

use ndarray::{ArrayD, IxDyn};
use ort::execution_providers::CPUExecutionProvider;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;

/// A loaded model with its input and output names.
///
/// The names are read from the graph rather than hard-coded, because ONNX exports
/// of the same architecture differ in naming (`input` vs `input_values`,
/// `output` vs `logits`) and a mismatch produces a runtime error deep inside
/// inference rather than at load time.
pub struct OnnxModel {
    session: Session,
    input_names: Vec<String>,
    output_names: Vec<String>,
    /// Custom metadata map, read from the file itself.
    ///
    /// Segmentation models describe their own window size and class layout here.
    /// Reading it from the file avoids hard-coding values that differ between
    /// exports and would otherwise silently stretch the timeline.
    custom_metadata: Vec<(String, String)>,
}

/// One output tensor, named so callers can look up what they need.
pub struct NamedOutput {
    pub name: String,
    pub values: ArrayD<f32>,
}

impl OnnxModel {
    /// Load a model from `path`.
    ///
    /// `intra_threads` of `None` lets ONNX Runtime choose, which is usually right;
    /// the diarisation path passes an explicit value only when the rest of the
    /// pipeline is already saturating the CPU.
    pub fn load(path: &Path, intra_threads: Option<usize>) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("Model file not found: {}", path.display()));
        }

        // Checked before touching `ort`. Creating a session with the runtime
        // uninitialised panics inside `ort` rather than returning an error, so
        // without this guard a missing or failed `onnxruntime.dll` would crash the
        // process instead of surfacing a message. The Parakeet loader does the
        // same thing for the same reason.
        crate::ensure_onnx_runtime_available()
            .map_err(|error| format!("ONNX Runtime is unavailable: {error}"))?;

        let mut builder = Session::builder()
            .map_err(|e| format!("Failed to create an ONNX session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("Failed to set the ONNX optimization level: {e}"))?
            .with_execution_providers([CPUExecutionProvider::default().build()])
            .map_err(|e| format!("Failed to set the ONNX execution provider: {e}"))?
            .with_parallel_execution(true)
            .map_err(|e| format!("Failed to enable ONNX parallel execution: {e}"))?;

        if let Some(threads) = intra_threads {
            builder = builder
                .with_intra_threads(threads)
                .map_err(|e| format!("Failed to set ONNX intra-op threads: {e}"))?
                .with_inter_threads(threads)
                .map_err(|e| format!("Failed to set ONNX inter-op threads: {e}"))?;
        }

        let session = builder
            .commit_from_file(path)
            .map_err(|e| format!("Failed to load ONNX model {}: {e}", path.display()))?;

        let input_names: Vec<String> = session.inputs.iter().map(|i| i.name.clone()).collect();
        let output_names: Vec<String> = session.outputs.iter().map(|o| o.name.clone()).collect();

        if input_names.is_empty() {
            return Err(format!(
                "ONNX model {} declares no inputs",
                path.display()
            ));
        }

        // Read the custom metadata map. A model without one is normal, so a failure
        // here is logged and treated as empty rather than fatal.
        let custom_metadata = match session.metadata() {
            Ok(metadata) => match metadata.custom_keys() {
                Ok(keys) => keys
                    .into_iter()
                    .filter_map(|key| {
                        metadata
                            .custom(&key)
                            .ok()
                            .flatten()
                            .map(|value| (key, value))
                    })
                    .collect(),
                Err(e) => {
                    log::debug!("Could not list ONNX metadata keys for {}: {e}", path.display());
                    Vec::new()
                }
            },
            Err(e) => {
                log::debug!("Could not read ONNX metadata for {}: {e}", path.display());
                Vec::new()
            }
        };

        log::info!(
            "Loaded ONNX model {} (inputs: {:?}, outputs: {:?}, metadata keys: {})",
            path.display(),
            input_names,
            output_names,
            custom_metadata.len()
        );

        Ok(Self {
            session,
            input_names,
            output_names,
            custom_metadata,
        })
    }

    /// The model's own custom metadata, as key/value pairs.
    pub fn custom_metadata(&self) -> &[(String, String)] {
        &self.custom_metadata
    }

    pub fn input_names(&self) -> &[String] {
        &self.input_names
    }

    pub fn output_names(&self) -> &[String] {
        &self.output_names
    }

    /// Number of inputs the graph expects, used to validate a call before making it.
    pub fn input_count(&self) -> usize {
        self.input_names.len()
    }

    /// Run inference with one float tensor per input, in the graph's own order.
    ///
    /// Every output is returned as a float array. Diarisation graphs output only
    /// floats, and narrowing that would force callers to re-handle the type case
    /// for no benefit.
    ///
    /// Takes `&mut self` because ONNX Runtime's `run` requires exclusive access,
    /// the same as the Parakeet engine's stages.
    pub fn run(&mut self, inputs: Vec<ArrayD<f32>>) -> Result<Vec<NamedOutput>, String> {
        if inputs.len() != self.input_names.len() {
            return Err(format!(
                "Model expects {} input(s) ({:?}) but {} were supplied",
                self.input_names.len(),
                self.input_names,
                inputs.len()
            ));
        }

        let refs: Vec<TensorRef<f32>> = inputs
            .iter()
            .map(|array| {
                TensorRef::from_array_view(array.view())
                    .map_err(|e| format!("Failed to prepare an ONNX input tensor: {e}"))
            })
            .collect::<Result<_, _>>()?;

        // `ort::inputs!` needs a fixed arity, so bind the positional tensors to the
        // graph's own names instead.
        let named: Vec<(&str, TensorRef<f32>)> = self
            .input_names
            .iter()
            .map(String::as_str)
            .zip(refs)
            .collect();

        let outputs = self
            .session
            .run(named)
            .map_err(|e| format!("ONNX inference failed: {e}"))?;

        let mut result = Vec::with_capacity(self.output_names.len());

        for name in &self.output_names {
            let value = outputs
                .get(name.as_str())
                .ok_or_else(|| format!("ONNX model produced no output named '{name}'"))?;

            let array = value
                .try_extract_array::<f32>()
                .map_err(|e| format!("ONNX output '{name}' is not a float tensor: {e}"))?
                .to_owned()
                .into_dyn();

            result.push(NamedOutput {
                name: name.clone(),
                values: array,
            });
        }

        Ok(result)
    }

    /// Run inference and return the first output.
    pub fn run_single(&mut self, inputs: Vec<ArrayD<f32>>) -> Result<ArrayD<f32>, String> {
        let mut outputs = self.run(inputs)?;
        if outputs.is_empty() {
            return Err("ONNX model produced no outputs".to_string());
        }
        Ok(outputs.remove(0).values)
    }
}

/// Build a 1-D float array.
pub fn array1(values: Vec<f32>) -> ArrayD<f32> {
    ArrayD::from_shape_vec(IxDyn(&[values.len()]), values)
        .expect("a flat vector always matches its own length")
}

/// Build a 2-D float array from row-major data.
pub fn array2(rows: usize, cols: usize, values: Vec<f32>) -> Result<ArrayD<f32>, String> {
    ArrayD::from_shape_vec(IxDyn(&[rows, cols]), values)
        .map_err(|e| format!("Failed to build a {rows}x{cols} tensor: {e}"))
}

/// Build a 3-D float array from row-major data.
pub fn array3(
    dim0: usize,
    dim1: usize,
    dim2: usize,
    values: Vec<f32>,
) -> Result<ArrayD<f32>, String> {
    ArrayD::from_shape_vec(IxDyn(&[dim0, dim1, dim2]), values)
        .map_err(|e| format!("Failed to build a {dim0}x{dim1}x{dim2} tensor: {e}"))
}

/// Resolve the directory holding the diarisation models.
///
/// Follows the same `<app data>/models/<engine>` layout as the transcription
/// models, so the existing storage-location setting and the model-download path
/// apply to them too.
pub fn diarization_model_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models").join("diarization")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_file_is_reported_by_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope.onnx");

        let error = OnnxModel::load(&missing, None)
            .err()
            .expect("a missing file must fail to load");
        assert!(error.contains("nope.onnx"), "got: {error}");
    }

    /// A non-model file must never load successfully.
    ///
    /// Whether it returns an error or panics is platform-dependent and outside
    /// this module's control: `ort` panics when its runtime is uninitialised, and
    /// `ensure_onnx_runtime_available` can only report an error on Windows. Both
    /// outcomes are therefore tolerated here; what must not happen is a successful
    /// load of a file that is not a graph.
    #[test]
    fn a_non_model_file_never_loads_successfully() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bogus = tmp.path().join("bogus.onnx");
        std::fs::write(&bogus, b"this is not an onnx graph").expect("write");

        let result = std::panic::catch_unwind(|| OnnxModel::load(&bogus, None));

        if let Ok(Ok(_)) = result {
            panic!("a non-model file must not load successfully");
        }
    }

    /// On Windows the runtime guard must report an error rather than letting
    /// `ort` panic, because a failed `onnxruntime.dll` load is a realistic
    /// condition on that platform and a panic would take the app down with it.
    #[cfg(target_os = "windows")]
    #[test]
    fn on_windows_a_bad_file_reports_an_error_without_panicking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bogus = tmp.path().join("bogus.onnx");
        std::fs::write(&bogus, b"this is not an onnx graph").expect("write");

        let outcome = std::panic::catch_unwind(|| OnnxModel::load(&bogus, None));

        match outcome {
            Ok(Err(_)) => {}
            Ok(Ok(_)) => panic!("a non-model file must not load successfully"),
            Err(_) => panic!(
                "the Windows runtime guard must report an error instead of panicking"
            ),
        }
    }

    #[test]
    fn array1_matches_its_input_length() {
        let array = array1(vec![1.0, 2.0, 3.0]);
        assert_eq!(array.shape(), &[3]);
        assert_eq!(array[[0]], 1.0);
    }

    #[test]
    fn array1_handles_an_empty_vector() {
        let array = array1(vec![]);
        assert_eq!(array.shape(), &[0]);
    }

    #[test]
    fn array2_builds_the_requested_shape() {
        let array = array2(2, 3, vec![0.0; 6]).expect("array2");
        assert_eq!(array.shape(), &[2, 3]);
    }

    #[test]
    fn array2_rejects_a_length_mismatch() {
        // A mismatch must be an error, not a silently reshaped tensor.
        assert!(array2(2, 3, vec![0.0; 5]).is_err());
    }

    #[test]
    fn array3_builds_the_requested_shape() {
        let array = array3(1, 2, 3, vec![0.0; 6]).expect("array3");
        assert_eq!(array.shape(), &[1, 2, 3]);
    }

    #[test]
    fn array3_rejects_a_length_mismatch() {
        assert!(array3(2, 2, 2, vec![0.0; 7]).is_err());
    }

    #[test]
    fn the_model_directory_follows_the_existing_layout() {
        // Kept under the same `models/<engine>` root so the storage-location
        // setting and the download path apply to these models too.
        let dir = diarization_model_dir(Path::new("C:/appdata"));
        assert!(dir.ends_with("models/diarization") || dir.ends_with("models\\diarization"));
    }
}
