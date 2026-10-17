// audio/simple_level_monitor.rs
//
// Hosts the real level monitor on a thread of its own.
//
// `level_monitor::AudioLevelMonitor` owns `cpal::Stream`, which is not `Send`: the
// streams cannot be moved between threads, and therefore cannot live in Tauri's
// managed state the way the other command handlers expect. That restriction is why
// this module once answered the level commands with a sine wave — a meter that moved
// whether or not a device was delivering anything, which is worse than no meter.
//
// The streams only need to be owned by *one* thread for their whole life, not by the
// main one. They are created, used and dropped inside a single closure here, and this
// module keeps the module-level API the commands already call, so nothing else has to
// change.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Result;
use log::{error, info};
use tauri::{AppHandle, Runtime};

/// Whether a monitor thread should keep running.
static IS_MONITORING: AtomicBool = AtomicBool::new(false);

/// Bumped on every start.
///
/// A thread from an earlier session reads this to notice it has been replaced even
/// when a new session has already set `IS_MONITORING` back to true, so two sets of
/// capture streams never run against the same device.
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Start monitoring the given devices, emitting `audio-levels` every 100 ms.
pub async fn start_monitoring<R: Runtime>(
    app_handle: AppHandle<R>,
    device_names: Vec<String>,
) -> Result<()> {
    // Ask any running monitor to stop and give it a moment to drop its streams.
    IS_MONITORING.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    IS_MONITORING.store(true, Ordering::SeqCst);

    info!(
        "Starting audio level monitoring for devices: {:?}",
        device_names
    );

    std::thread::Builder::new()
        .name("audio-level-monitor".to_string())
        .spawn(move || {
            // A current-thread runtime is enough: the monitor needs timers and one
            // task, and this keeps it independent of the application's main runtime.
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    error!("Could not start the audio level monitor runtime: {e}");
                    IS_MONITORING.store(false, Ordering::SeqCst);
                    return;
                }
            };

            // Everything that touches a `cpal::Stream` stays inside this block, so the
            // streams never cross a thread boundary.
            runtime.block_on(async move {
                let mut monitor = super::level_monitor::AudioLevelMonitor::new();

                if let Err(e) = monitor.start_monitoring(app_handle, device_names).await {
                    error!("Failed to start audio level monitoring: {e}");
                    IS_MONITORING.store(false, Ordering::SeqCst);
                    return;
                }

                // Hold the monitor — and keep driving the emission task that
                // `start_monitoring` spawned — until a stop is requested.
                while IS_MONITORING.load(Ordering::SeqCst)
                    && GENERATION.load(Ordering::SeqCst) == generation
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }

                let _ = monitor.stop_monitoring().await;
            });

            info!("Audio level monitor thread finished");
        })
        .map_err(|e| anyhow::anyhow!("Could not spawn the audio level monitor thread: {e}"))?;

    Ok(())
}

/// Stop monitoring; the monitor thread drops its streams and exits.
pub async fn stop_monitoring() -> Result<()> {
    info!("Stopping audio level monitoring");
    IS_MONITORING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Check if currently monitoring.
pub fn is_monitoring() -> bool {
    IS_MONITORING.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A start followed by a stop must end up not monitoring. This cannot exercise
    /// the audio path itself — there may be no device in the test environment — but
    /// it does cover the state machine the commands report on.
    #[test]
    fn stopping_clears_the_monitoring_flag() {
        IS_MONITORING.store(true, Ordering::SeqCst);
        assert!(is_monitoring());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(stop_monitoring()).expect("stop");

        assert!(!is_monitoring());
    }

    /// A new session must invalidate the previous one, or the old thread would keep
    /// streaming until the flag happened to go false.
    #[test]
    fn a_new_generation_invalidates_the_previous_one() {
        let first = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let second = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

        assert_ne!(first, second);
        assert_ne!(GENERATION.load(Ordering::SeqCst), first);
    }
}
