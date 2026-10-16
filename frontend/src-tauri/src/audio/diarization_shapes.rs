//! Prints the tensor shapes of the diarisation models.
//!
//! Ignored by default; run with:
//! `cargo test -p meetily --lib diarization_shapes -- --ignored --nocapture`
//!
//! Kept as a test because the shapes are what the wiring depends on, and guessing
//! them produced two wrong attempts (a rank-2 input, then a 160000-sample axis where
//! the model wanted 80 mel bins).

#![cfg(test)]

use std::path::PathBuf;

fn models_dir() -> PathBuf {
    std::env::var("MEETILY_DIARIZATION_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"D:\Meetly\Model\diarization"))
}

/// Describe one model file.
fn describe(label: &str, path: &PathBuf) {
    if !path.is_file() {
        println!("{label}: missing at {}", path.display());
        return;
    }

    let mut builder = ort::session::Session::builder().expect("session builder");
    builder = builder
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .expect("optimization level");
    builder = builder
        .with_execution_providers([ort::execution_providers::CPUExecutionProvider::default().build()])
        .expect("provider");

    let session = builder.commit_from_file(path).expect("load model");

    println!("\n=== {label} ===");
    println!("file: {}", path.display());

    for input in &session.inputs {
        println!("  input  {:<12} {:?}", input.name, input.input_type);
    }
    for output in &session.outputs {
        println!("  output {:<12} {:?}", output.name, output.output_type);
    }

    // Bind the metadata first: `ModelMetadata` borrows the session, and a temporary
    // in the `match` scrutinee is dropped while still borrowed.
    let metadata_result = session.metadata();

    match metadata_result {
        Ok(metadata) => match metadata.custom_keys() {
            Ok(keys) => {
                if keys.is_empty() {
                    println!("  metadata: (none)");
                } else {
                    println!("  metadata:");
                    for key in keys {
                        let value = metadata.custom(&key).ok().flatten().unwrap_or_default();
                        println!("    {key} = {value}");
                    }
                }
            }
            Err(e) => println!("  metadata keys unavailable: {e}"),
        },
        Err(e) => println!("  metadata unavailable: {e}"),
    }
}

#[test]
#[ignore = "prints model tensor shapes; run with --ignored --nocapture"]
fn describe_diarization_model_shapes() {
    #[cfg(target_os = "windows")]
    {
        let dll = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("onnxruntime")
            .join("onnxruntime.dll");
        let _ = ort::init_from(dll.to_string_lossy().into_owned())
            .with_telemetry(false)
            .commit();
        crate::record_onnx_runtime_ready();
    }

    let dir = models_dir();
    describe(
        "segmentation (pyannote 3.0)",
        &dir.join("sherpa-onnx-pyannote-segmentation-3-0").join("model.onnx"),
    );
    describe("embedding (3dspeaker eres2net zh)", &dir.join("3dspeaker_eres2net_zh.onnx"));
}
