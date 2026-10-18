// audio/process_capture.rs
//
// Capturing one application's audio instead of the whole system mix.
//
// cpal, which the rest of the capture path uses, only exposes the default device's mix.
// Process-scoped capture is a different WASAPI activation: the caller asks for
// `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` with a target process id and a scope, and
// Windows returns an `IAudioClient` carrying only that application's audio.
//
// The activation is asynchronous — `ActivateAudioInterfaceAsync` hands the client to a
// COM callback — so this module bridges that callback back into a blocking call. That is
// why it lives apart from `app_audio.rs`, which only reads ids and names.

/// Format every capture stream is requested in: 48 kHz, mono, f32.
///
/// The rest of the pipeline already expects 48 kHz — Bluetooth devices are resampled up
/// to it — so asking WASAPI for the same rate avoids a second resampler in the path.
pub const CAPTURE_SAMPLE_RATE: u32 = 48_000;

#[cfg(target_os = "windows")]
pub use imp::start_process_capture;

/// Start feeding one application's audio into the running recording pipeline.
///
/// Samples are delivered as `DeviceType::System` chunks, which is what the mixing stage
/// expects from the system side of a meeting — so choosing an application instead of a
/// device gets the same downstream behaviour, including the mix with the microphone.
///
/// The capture runs on its own thread and stops when recording stops. Because the chunks
/// are typed as system audio, a caller must not also ask for a system *device*, or the two
/// streams would be summed together.
#[cfg(target_os = "windows")]
pub fn spawn_process_capture(
    state: std::sync::Arc<crate::audio::recording_state::RecordingState>,
    process_id: u32,
) -> Result<(), String> {
    use crate::audio::recording_state::{AudioChunk, DeviceType};
    use std::time::Duration;

    // The session decides once, before recording starts, whether the file is mono or
    // stereo; asking WASAPI for stereo only when the recorder writes stereo keeps the
    // microphone-involving paths byte-identical to before.
    let channels = state.recording_channels();

    std::thread::Builder::new()
        .name("app-audio-capture".to_string())
        .spawn(move || {
            let capture = match start_process_capture(process_id, channels) {
                Ok(capture) => capture,
                Err(e) => {
                    // Reported through the log rather than the recording error channel:
                    // a failed per-app source means no system audio, not a broken session,
                    // and the microphone keeps recording.
                    log::error!("Could not capture application {process_id}: {e}");
                    return;
                }
            };

            log::info!(
                "Capturing application {process_id} as system audio ({channels} channel(s))"
            );
            let mut chunk_id: u64 = 0;

            // Keep draining while paused: the buffer would otherwise overflow and
            // replay stale audio on resume. send_audio_chunk already discards chunks
            // while paused, so only the recording flag ends the loop.
            while state.is_recording() {
                match capture.read() {
                    Ok(samples) if !samples.is_empty() => {
                        let chunk = AudioChunk {
                            data: samples,
                            sample_rate: CAPTURE_SAMPLE_RATE,
                            timestamp: state.get_recording_duration().unwrap_or(0.0),
                            chunk_id,
                            device_type: DeviceType::System,
                        };
                        chunk_id = chunk_id.wrapping_add(1);
                        if state.send_audio_chunk(chunk).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log::error!("Application audio capture stopped: {e}");
                        break;
                    }
                }
                // A 20 ms poll keeps latency below what a listener would notice while
                // leaving the thread idle most of the time.
                std::thread::sleep(Duration::from_millis(20));
            }

            log::info!("Application audio capture finished after {chunk_id} chunks");
        })
        .map_err(|e| format!("could not start the application capture thread: {e}"))?;

    Ok(())
}

#[cfg(target_os = "windows")]
mod imp {
    use super::CAPTURE_SAMPLE_RATE;
    use std::sync::mpsc;
    use windows::core::{implement, Interface};
    use windows::Win32::Media::Audio::{
        ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
        IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED, AUDIOCLIENT_ACTIVATION_PARAMS,
        AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Variant::VT_BLOB;

    /// Event-driven capture, which a loopback client requires.
    const STREAM_FLAGS: u32 = 0x0002_0000 | 0x8000_0000;

    /// Bridges the asynchronous activation callback into a channel.
    ///
    /// `ActivateCompleted` runs on a COM thread once the interface is ready; the caller
    /// waits on the receiving half, which turns the callback back into a blocking call.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationBridge {
        sender: std::sync::Mutex<Option<mpsc::Sender<Result<IAudioClient, String>>>>,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationBridge_Impl {
        fn ActivateCompleted(
            &self,
            activateoperation: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            let outcome = (|| -> Result<IAudioClient, String> {
                let operation = activateoperation
                    .as_ref()
                    .ok_or_else(|| "activation completed without an operation".to_string())?;

                let mut activate_result = windows::core::HRESULT(0);
                let mut interface: Option<windows::core::IUnknown> = None;
                unsafe { operation.GetActivateResult(&mut activate_result, &mut interface) }
                    .map_err(|e| format!("could not read the activation result: {e}"))?;

                if activate_result.is_err() {
                    return Err(format!(
                        "process loopback activation failed with 0x{:08X}",
                        activate_result.0
                    ));
                }

                let interface =
                    interface.ok_or_else(|| "the activation returned no interface".to_string())?;
                interface
                    .cast::<IAudioClient>()
                    .map_err(|e| format!("the activation did not return an audio client: {e}"))
            })();

            if let Ok(mut guard) = self.sender.lock() {
                if let Some(sender) = guard.take() {
                    let _ = sender.send(outcome);
                }
            }
            Ok(())
        }
    }

    /// An open capture stream for one application.
    pub struct ProcessCapture {
        /// Held so the stream stays open; dropping it stops capture.
        _client: IAudioClient,
        capture: IAudioCaptureClient,
        /// Interleaved channel count requested from WASAPI (1 or 2).
        channels: u16,
    }

    impl ProcessCapture {
        /// Read whatever the application has produced since the last call.
        ///
        /// An empty vector is the normal answer between two polls.
        pub fn read(&self) -> Result<Vec<f32>, String> {
            let mut samples: Vec<f32> = Vec::new();

            loop {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                unsafe {
                    self.capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(|e| format!("could not read captured audio: {e}"))?;
                }

                if frames == 0 {
                    break;
                }

                // The requested format is f32 interleaved, so the buffer holds
                // `frames * channels` samples.
                // 0x2 is AUDCLNT_BUFFERFLAGS_SILENT: a silent packet carries no audio, and
                // keeping it would look like the application went quiet.
                let silent = flags & 0x2 != 0;
                if !data.is_null() && !silent {
                    let sample_count = frames as usize * self.channels as usize;
                    let slice =
                        unsafe { std::slice::from_raw_parts(data as *const f32, sample_count) };
                    samples.extend_from_slice(slice);
                }

                unsafe {
                    self.capture
                        .ReleaseBuffer(frames)
                        .map_err(|e| format!("could not release an audio buffer: {e}"))?;
                }
            }

            Ok(samples)
        }
    }

    /// Build the `PROPVARIANT` the activation API expects for process loopback.
    ///
    /// The API reads this as a `VT_BLOB` whose bytes *are* an
    /// `AUDIOCLIENT_ACTIVATION_PARAMS`. The nesting is not stylistic: `PROPVARIANT` is a
    /// union of a struct that itself contains a union, and `blob` sits two levels down.
    /// The size is computed from the struct rather than written out, because a wrong
    /// `cbSize` fails deep inside WASAPI with a code that says nothing useful.
    ///
    /// `params` must stay alive until the activation has been started.
    pub(super) fn activation_variant(params: &AUDIOCLIENT_ACTIVATION_PARAMS) -> PROPVARIANT {
        use windows::Win32::System::Com::StructuredStorage::PROPVARIANT_0_0;

        let mut variant = PROPVARIANT::default();
        unsafe {
            // `PROPVARIANT_0_0` is stored inside a `ManuallyDrop`, which derefs but does
            // not propagate field assignment through a union access, so it is taken as a
            // reference first.
            let size = std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>();
            // `VT_BLOB` means "CoTaskMemAlloc'd buffer", and COM may call
            // `PropVariantClear` on it. Pointing `pBlobData` at a stack local makes that
            // clear free a stack pointer, which is heap corruption rather than an error.
            let memory = windows::Win32::System::Com::CoTaskMemAlloc(size);
            if memory.is_null() {
                return variant;
            }
            std::ptr::copy_nonoverlapping(
                params as *const AUDIOCLIENT_ACTIVATION_PARAMS as *const u8,
                memory as *mut u8,
                size,
            );

            let inner: &mut PROPVARIANT_0_0 = &mut variant.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob.cbSize = size as u32;
            inner.Anonymous.blob.pBlobData = memory as *mut u8;
        }
        variant
    }

    /// Start capturing one application's audio.
    ///
    /// `process_id` should come from `app_audio::list_audio_apps`. The tree scope includes
    /// audio played by that process's children, which is what makes a meeting app whose
    /// playback lives in a helper process actually recordable.
    pub fn start_process_capture(process_id: u32, channels: u16) -> Result<ProcessCapture, String> {
        let channels = channels.clamp(1, 2);
        // The activation needs COM on the calling thread, and on Windows the callback is
        // delivered through a multi-threaded apartment. Without this the call is made on
        // an uninitialised apartment and corrupts the heap rather than returning an error.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: process_id,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };

        let (sender, receiver) = mpsc::channel();
        let bridge = ActivationBridge {
            sender: std::sync::Mutex::new(Some(sender)),
        };
        let handler: IActivateAudioInterfaceCompletionHandler = bridge.into();
        let variant = activation_variant(&params);

        // The operation must outlive the callback; holding it here keeps it alive until
        // the client has arrived.
        let _operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&variant),
                &handler,
            )
            .map_err(|e| format!("could not start process loopback activation: {e}"))?
        };

        let client = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "the process loopback activation timed out".to_string())??;
        // Process loopback clients take a plain PCM format and do not use a shared-mode
        // period, unlike device clients.
        let block_align = 4u16 * channels;
        let mut format = windows::Win32::Media::Audio::WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: channels,
            nSamplesPerSec: CAPTURE_SAMPLE_RATE,
            wBitsPerSample: 32,
            nBlockAlign: block_align,
            nAvgBytesPerSec: CAPTURE_SAMPLE_RATE * block_align as u32,
            cbSize: 0,
        };

        unsafe {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    STREAM_FLAGS,
                    0,
                    0,
                    &mut format,
                    None,
                )
                .map_err(|e| format!("could not initialise the capture client: {e}"))?;

            let capture: IAudioCaptureClient = client
                .GetService()
                .map_err(|e| format!("could not get the capture client: {e}"))?;

            client
                .Start()
                .map_err(|e| format!("could not start capturing: {e}"))?;

            Ok(ProcessCapture {
                _client: client,
                capture,
                channels,
            })
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    /// Capture a second of audio from whatever is currently playing.
    ///
    /// Ignored by default because it needs a real application producing sound; run it
    /// with `--ignored --nocapture` while something is playing to check the whole
    /// activation path end to end.
    #[test]
    #[ignore]
    fn captures_audio_from_a_playing_application() {
        let apps = crate::audio::app_audio::list_audio_apps().expect("list audio apps");
        let target = apps.first().expect("nothing is playing audio right now");
        println!("capturing from {} (pid {})", target.display_name, target.process_ids[0]);

        let capture = start_process_capture(target.process_ids[0], 1).expect("start capture");
        println!("capture stream started");


        let mut total = 0usize;
        let mut non_silent = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

        while std::time::Instant::now() < deadline {
            match capture.read() {
                Ok(samples) => {
                    if !samples.is_empty() {
                        total += samples.len();
                        if samples.iter().any(|s| s.abs() > 1e-4) {
                            non_silent += samples.len();
                        }
                    }
                }
                Err(e) => panic!("read failed: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        println!("captured {total} samples, {non_silent} non-silent");
        assert!(total > 0, "no audio arrived — the activation produced a dead stream");
        assert!(
            non_silent > 0,
            "audio arrived but was entirely silent at 48kHz mono f32"
        );
    }
}

#[cfg(all(test, target_os = "windows"))]
mod layout_probe {
    use super::imp::activation_variant;
    use windows::Win32::Media::Audio::{
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    };

    fn params() -> AUDIOCLIENT_ACTIVATION_PARAMS {
        AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: 4242,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        }
    }

    #[test]
    fn propvariant_layout_is_what_wasapi_expects() {
        let p = params();
        let v = activation_variant(&p);

        // x64 Win32 PROPVARIANT: vt(2) + 3 reserved(2 each) = 8, then a 16-byte union.
        assert_eq!(std::mem::size_of_val(&v), 24, "PROPVARIANT size is not 24 on x64");
        assert_eq!(
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>(),
            12,
            "activation params are activation type(4) + pid(4) + mode(4)"
        );

        // Read the blob back through a raw pointer, which is exactly how WASAPI reads it.
        unsafe {
            let base = &v as *const _ as *const u8;
            let vt = *(base as *const u16);
            let cb_size = *(base.add(8) as *const u32);
            let data = *(base.add(16) as *const *const u8);

            println!("vt={vt} cbSize={cb_size} data_is_params={}", data == &p as *const _ as *const u8);

            assert_eq!(vt, 65, "vt should be VT_BLOB(65)");
            assert_eq!(cb_size as usize, std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>());
            assert_eq!(data, &p as *const _ as *const u8, "blob must point at the params");
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod integration_tests {
    use super::*;
    use crate::audio::recording_state::{DeviceType, RecordingState};

    /// Drive a real recording state with a real capture and check that chunks arrive.
    ///
    /// Ignored by default: it needs an application actively playing sound. Run with
    /// `--ignored --nocapture` while audio is playing.
    #[test]
    #[ignore]
    fn application_audio_reaches_the_recording_pipeline() {
        let apps = crate::audio::app_audio::list_audio_apps().expect("list audio apps");
        let target = apps.first().expect("nothing is playing audio right now");
        println!("target: {} (pid {})", target.display_name, target.process_ids[0]);

        let state = RecordingState::new();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        state.set_audio_sender(sender);
        state.start_recording().expect("start recording");

        spawn_process_capture(state.clone(), target.process_ids[0]).expect("spawn capture");

        let mut chunks = 0usize;
        let mut samples = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

        while std::time::Instant::now() < deadline {
            match receiver.try_recv() {
                Ok(chunk) => {
                    assert_eq!(chunk.sample_rate, CAPTURE_SAMPLE_RATE);
                    assert!(matches!(chunk.device_type, DeviceType::System));
                    chunks += 1;
                    samples += chunk.data.len();
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }

        state.stop_recording();
        println!("chunks={chunks} samples={samples}");

        assert!(chunks > 0, "no chunks reached the pipeline");
        assert!(
            samples >= CAPTURE_SAMPLE_RATE as usize / 2,
            "less than half a second of audio arrived: {samples} samples"
        );
    }
}