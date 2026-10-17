//! How long a local speech model stays in memory after it was used.
//!
//! Loading a Whisper / Parakeet model is the slowest step of starting a
//! recording (seconds for the larger models), while keeping one loaded costs
//! hundreds of MB to a few GB of RAM. The original behaviour was "unload as
//! soon as a recording or batch job ends", which made every start pay the full
//! load. This module turns that into an explicit policy:
//!
//! | policy     | after use                         | start latency | memory |
//! |------------|-----------------------------------|---------------|--------|
//! | `onDemand` | unload immediately (old behaviour)| slow          | lowest |
//! | `keepWarm` | unload after N idle minutes       | fast if recent| medium |
//! | `always`   | preload at launch, never unload   | fastest       | highest|
//!
//! `keepWarm` also preloads in the background shortly after launch (bounded by
//! the same idle timeout), and any policy preloads when the user points at the
//! record button (`transcription_prewarm`).
//! | `auto`     | `keepWarm` on ≥16 GB RAM, else `onDemand` |       |        |
//!
//! Two further memory rules apply whatever the policy:
//! * only one local engine is resident at a time (switching Whisper ⇄ Parakeet,
//!   or to a cloud provider, releases the other one);
//! * an idle model is released before the built-in summary model starts, so the
//!   two large models are not held at the same time after a meeting.

use log::{info, warn};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Runtime};

pub const DEFAULT_KEEP_WARM_MINUTES: u32 = 10;
const MAX_KEEP_WARM_MINUTES: u32 = 240;
/// Machines sold as 16 GB report a little less than 16 GiB.
const AUTO_KEEP_WARM_MIN_RAM_BYTES: u64 = 15 * 1024 * 1024 * 1024;
/// A model preloaded under `onDemand` (by hovering the record button) that is
/// then not used is released after this long.
const ON_DEMAND_PREWARM_GRACE_SECS: u64 = 120;
const REAPER_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Residency {
    OnDemand,
    KeepWarm { minutes: u32 },
    Always,
}

impl Residency {
    pub fn id(&self) -> &'static str {
        match self {
            Residency::OnDemand => "onDemand",
            Residency::KeepWarm { .. } => "keepWarm",
            Residency::Always => "always",
        }
    }
}

/// Resolve the stored preference into an effective policy.
pub fn resolve(policy: &str, minutes: u32, total_ram_bytes: Option<u64>) -> Residency {
    let minutes = if minutes == 0 {
        DEFAULT_KEEP_WARM_MINUTES
    } else {
        minutes.min(MAX_KEEP_WARM_MINUTES)
    };
    match policy {
        "onDemand" => Residency::OnDemand,
        "keepWarm" => Residency::KeepWarm { minutes },
        "always" => Residency::Always,
        // "auto" and anything unknown
        _ => {
            if total_ram_bytes.map_or(false, |r| r >= AUTO_KEEP_WARM_MIN_RAM_BYTES) {
                Residency::KeepWarm { minutes }
            } else {
                Residency::OnDemand
            }
        }
    }
}

/// Pure decision used by the idle reaper.
pub fn should_release_idle(policy: Residency, idle_secs: u64, busy: bool) -> bool {
    if busy {
        return false;
    }
    match policy {
        Residency::OnDemand => idle_secs >= ON_DEMAND_PREWARM_GRACE_SECS,
        Residency::KeepWarm { minutes } => idle_secs >= u64::from(minutes) * 60,
        Residency::Always => false,
    }
}

static LAST_USED: AtomicU64 = AtomicU64::new(0);
static ACTIVE_JOBS: AtomicUsize = AtomicUsize::new(0);
static PREWARMING: AtomicBool = AtomicBool::new(false);
static REAPER_STARTED: AtomicBool = AtomicBool::new(false);
static POLICY_ALWAYS: AtomicBool = AtomicBool::new(false);

static TOTAL_RAM: Lazy<Option<u64>> = Lazy::new(|| {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let total = sys.total_memory();
    (total > 0).then_some(total)
});

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Mark the local model as just used.
pub fn touch() {
    LAST_USED.store(now_secs(), Ordering::SeqCst);
}

fn idle_secs() -> u64 {
    let last = LAST_USED.load(Ordering::SeqCst);
    if last == 0 {
        return u64::MAX;
    }
    now_secs().saturating_sub(last)
}

pub fn total_memory_bytes() -> Option<u64> {
    *TOTAL_RAM
}

/// Held for the duration of a batch job (import / retranscription) so the idle
/// reaper never unloads a model that is mid-transcription.
pub struct ModelUseGuard(());

impl Drop for ModelUseGuard {
    fn drop(&mut self) {
        ACTIVE_JOBS.fetch_sub(1, Ordering::SeqCst);
        touch();
    }
}

pub fn begin_use() -> ModelUseGuard {
    ACTIVE_JOBS.fetch_add(1, Ordering::SeqCst);
    touch();
    ModelUseGuard(())
}

/// True when the calling batch job is the only one holding the model.
pub fn is_sole_user() -> bool {
    ACTIVE_JOBS.load(Ordering::SeqCst) <= 1
}

/// True when no batch job holds the model (for callers outside a job).
pub fn no_active_jobs() -> bool {
    ACTIVE_JOBS.load(Ordering::SeqCst) == 0
}

async fn is_busy() -> bool {
    crate::audio::recording_commands::is_recording().await
        || ACTIVE_JOBS.load(Ordering::SeqCst) > 0
        || PREWARMING.load(Ordering::SeqCst)
}

pub async fn current_policy<R: Runtime>(app: &AppHandle<R>) -> Residency {
    let policy = match crate::audio::recording_preferences::load_recording_preferences(app).await {
        Ok(prefs) => resolve(
            &prefs.model_residency,
            prefs.model_keep_warm_minutes,
            total_memory_bytes(),
        ),
        Err(_) => resolve("auto", DEFAULT_KEEP_WARM_MINUTES, total_memory_bytes()),
    };
    POLICY_ALWAYS.store(policy == Residency::Always, Ordering::SeqCst);
    policy
}

fn whisper() -> Option<std::sync::Arc<crate::whisper_engine::WhisperEngine>> {
    let guard = crate::whisper_engine::commands::WHISPER_ENGINE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.as_ref().cloned()
}

fn parakeet() -> Option<std::sync::Arc<crate::parakeet_engine::ParakeetEngine>> {
    let guard = crate::parakeet_engine::commands::PARAKEET_ENGINE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.as_ref().cloned()
}

/// Unload every resident local model except the one for `keep`
/// (`"localWhisper"` / `"parakeet"`; anything else releases both).
/// Returns the names of the models that were released.
pub async fn release_engines_except(keep: Option<&str>) -> Vec<String> {
    let mut released = Vec::new();
    if keep != Some("localWhisper") {
        if let Some(engine) = whisper() {
            if engine.is_model_loaded().await {
                let name = engine.get_current_model().await.unwrap_or_default();
                if engine.unload_model().await {
                    released.push(format!("whisper:{}", name));
                }
            }
        }
    }
    if keep != Some("parakeet") {
        if let Some(engine) = parakeet() {
            if engine.is_model_loaded().await {
                let name = engine.get_current_model().await.unwrap_or_default();
                if engine.unload_model().await {
                    released.push(format!("parakeet:{}", name));
                }
            }
        }
    }
    if !released.is_empty() {
        info!("🧹 Released speech model(s) from memory: {:?}", released);
    }
    released
}

/// Called when a recording or batch job has finished with the model.
///
/// Replaces the old unconditional unload: under `keepWarm` / `always` the model
/// stays loaded so the next recording starts immediately; the idle reaper
/// releases it later.
pub async fn release_after_use<R: Runtime>(app: &AppHandle<R>) {
    touch();
    let policy = current_policy(app).await;
    match policy {
        Residency::OnDemand => {
            if crate::audio::recording_commands::is_recording().await
                || ACTIVE_JOBS.load(Ordering::SeqCst) > 0
            {
                info!("Keeping speech model: still in use by another task");
                return;
            }
            release_engines_except(None).await;
        }
        Residency::KeepWarm { minutes } => {
            info!("⚡ Keeping speech model warm for {} min (faster next start)", minutes);
        }
        Residency::Always => info!("⚡ Speech model stays resident (policy: always)"),
    }
}

/// Release an idle local model before another large model is loaded (the
/// built-in summary LLM), so both are not held at once after a meeting.
/// Skipped under `always` and whenever the model is in use. Needs no app
/// handle: the policy last resolved (the reaper refreshes it every 30 s) is used.
pub async fn release_for_heavy_task(reason: &str) {
    if POLICY_ALWAYS.load(Ordering::SeqCst) || is_busy().await {
        return;
    }
    let Ok(_lock) = crate::audio::common::try_acquire_engine_lifecycle_lock() else {
        return;
    };
    if is_busy().await {
        return;
    }
    let released = release_engines_except(None).await;
    if !released.is_empty() {
        info!("Released speech model before {} to lower peak memory", reason);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PrewarmOutcome {
    /// `loaded`, `alreadyLoaded`, `skipped`, `failed`
    pub status: String,
    pub provider: String,
    pub detail: Option<String>,
}

/// Load the configured local model ahead of time so pressing "record" does not
/// wait for it. Cheap to call repeatedly: concurrent and redundant calls return
/// immediately.
pub async fn prewarm<R: Runtime>(app: &AppHandle<R>) -> PrewarmOutcome {
    let provider = super::engine::configured_provider_id(app).await;
    let outcome = |status: &str, detail: Option<String>| PrewarmOutcome {
        status: status.to_string(),
        provider: provider.clone(),
        detail,
    };

    if provider != "localWhisper" && provider != "parakeet" {
        return outcome("skipped", Some("cloud provider: nothing to load".into()));
    }
    if crate::audio::recording_commands::is_recording().await {
        return outcome("skipped", Some("recording in progress".into()));
    }
    if PREWARMING.swap(true, Ordering::SeqCst) {
        return outcome("skipped", Some("already preloading".into()));
    }

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PREWARMING.store(false, Ordering::SeqCst);
        }
    }
    let _reset = Reset;

    let already = match provider.as_str() {
        "parakeet" => match parakeet() {
            Some(e) => e.is_model_loaded().await,
            None => false,
        },
        _ => match whisper() {
            Some(e) => e.is_model_loaded().await,
            None => false,
        },
    };
    if already {
        touch();
        return outcome("alreadyLoaded", None);
    }

    // Holding the lifecycle lock makes a recording start that arrives mid-load
    // wait for this load instead of starting a second one.
    let _lock = crate::audio::common::acquire_engine_lifecycle_lock().await;
    if crate::audio::recording_commands::is_recording().await {
        return outcome("skipped", Some("recording in progress".into()));
    }
    let started = std::time::Instant::now();
    match super::engine::validate_transcription_model_ready(app).await {
        Ok(()) => {
            touch();
            let ms = started.elapsed().as_millis();
            info!("⚡ Preloaded speech model for '{}' in {} ms", provider, ms);
            outcome("loaded", Some(format!("{} ms", ms)))
        }
        Err(e) => {
            warn!("Speech model preload skipped: {}", e);
            outcome("failed", Some(e))
        }
    }
}

/// Background loop releasing idle models according to the policy.
pub fn start_idle_reaper<R: Runtime>(app: AppHandle<R>) {
    if REAPER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(REAPER_INTERVAL_SECS)).await;

            let policy = current_policy(&app).await;
            if !should_release_idle(policy, idle_secs(), is_busy().await) {
                continue;
            }
            // Never race a recording start or a batch job's own unload.
            let Ok(_lock) = crate::audio::common::try_acquire_engine_lifecycle_lock() else {
                continue;
            };
            if is_busy().await {
                continue;
            }
            let released = release_engines_except(None).await;
            if !released.is_empty() {
                info!(
                    "💤 Speech model idle for {}s under '{}' policy; released to free memory",
                    idle_secs(),
                    policy.id()
                );
            }
        }
    });
}

/// Called once at launch: starts the reaper and, unless `onDemand`, preloads
/// the model in the background a few seconds after the window is up.
pub fn on_app_start<R: Runtime>(app: AppHandle<R>) {
    start_idle_reaper(app.clone());
    tauri::async_runtime::spawn(async move {
        // Let the window and database settle first; preloading competes for disk.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        // Opening the app usually means a recording is coming. Under `keepWarm`
        // the preloaded model is still released after the idle period, so the
        // memory cost is bounded; `onDemand` never preloads.
        match current_policy(&app).await {
            Residency::Always | Residency::KeepWarm { .. } => {
                let result = prewarm(&app).await;
                info!("Startup preload: {} ({:?})", result.status, result.detail);
            }
            Residency::OnDemand => {}
        }
    });
}

#[derive(Debug, Clone, Serialize)]
pub struct ResidencyStatus {
    /// Stored preference (`auto`, `onDemand`, `keepWarm`, `always`).
    pub preference: String,
    /// Effective policy after resolving `auto`.
    pub effective: String,
    pub keep_warm_minutes: u32,
    pub total_memory_gb: Option<f64>,
    pub whisper_model: Option<String>,
    pub parakeet_model: Option<String>,
    pub idle_seconds: Option<u64>,
}

#[tauri::command]
pub async fn transcription_prewarm<R: Runtime>(app: AppHandle<R>) -> Result<PrewarmOutcome, String> {
    Ok(prewarm(&app).await)
}

#[tauri::command]
pub async fn get_model_residency_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ResidencyStatus, String> {
    let prefs = crate::audio::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|e| e.to_string())?;
    let effective = resolve(
        &prefs.model_residency,
        prefs.model_keep_warm_minutes,
        total_memory_bytes(),
    );
    let whisper_model = match whisper() {
        Some(e) if e.is_model_loaded().await => e.get_current_model().await,
        _ => None,
    };
    let parakeet_model = match parakeet() {
        Some(e) if e.is_model_loaded().await => e.get_current_model().await,
        _ => None,
    };
    let idle = idle_secs();
    Ok(ResidencyStatus {
        preference: prefs.model_residency.clone(),
        effective: effective.id().to_string(),
        keep_warm_minutes: match effective {
            Residency::KeepWarm { minutes } => minutes,
            _ => prefs.model_keep_warm_minutes,
        },
        total_memory_gb: total_memory_bytes()
            .map(|b| (b as f64 / 1024f64.powi(3) * 10.0).round() / 10.0),
        whisper_model,
        parakeet_model,
        idle_seconds: (idle != u64::MAX).then_some(idle),
    })
}

/// Release resident speech models now (settings "free memory" button).
#[tauri::command]
pub async fn release_speech_models() -> Result<Vec<String>, String> {
    if is_busy().await {
        return Err("The speech model is in use by a recording or transcription".into());
    }
    let _lock = crate::audio::common::acquire_engine_lifecycle_lock().await;
    Ok(release_engines_except(None).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn auto_keeps_warm_only_with_enough_memory() {
        assert_eq!(resolve("auto", 10, Some(32 * GB)), Residency::KeepWarm { minutes: 10 });
        assert_eq!(resolve("auto", 10, Some(15 * GB + GB / 2)), Residency::KeepWarm { minutes: 10 });
        assert_eq!(resolve("auto", 10, Some(8 * GB)), Residency::OnDemand);
        assert_eq!(resolve("auto", 10, None), Residency::OnDemand);
        assert_eq!(resolve("", 10, Some(8 * GB)), Residency::OnDemand);
    }

    #[test]
    fn explicit_policies_ignore_memory() {
        assert_eq!(resolve("onDemand", 10, Some(64 * GB)), Residency::OnDemand);
        assert_eq!(resolve("always", 10, Some(4 * GB)), Residency::Always);
        assert_eq!(resolve("keepWarm", 30, Some(4 * GB)), Residency::KeepWarm { minutes: 30 });
    }

    #[test]
    fn keep_warm_minutes_are_bounded() {
        assert_eq!(resolve("keepWarm", 0, None), Residency::KeepWarm { minutes: DEFAULT_KEEP_WARM_MINUTES });
        assert_eq!(resolve("keepWarm", 10_000, None), Residency::KeepWarm { minutes: MAX_KEEP_WARM_MINUTES });
    }

    #[test]
    fn reaper_never_releases_a_busy_model() {
        for policy in [Residency::OnDemand, Residency::KeepWarm { minutes: 1 }, Residency::Always] {
            assert!(!should_release_idle(policy, u64::MAX, true));
        }
    }

    #[test]
    fn reaper_follows_the_policy_when_idle() {
        let warm = Residency::KeepWarm { minutes: 10 };
        assert!(!should_release_idle(warm, 9 * 60, false));
        assert!(should_release_idle(warm, 10 * 60, false));
        assert!(!should_release_idle(Residency::Always, u64::MAX, false));
        assert!(!should_release_idle(Residency::OnDemand, 30, false));
        assert!(should_release_idle(Residency::OnDemand, ON_DEMAND_PREWARM_GRACE_SECS, false));
    }

    #[test]
    fn use_guard_tracks_active_jobs() {
        let before = ACTIVE_JOBS.load(Ordering::SeqCst);
        {
            let _a = begin_use();
            let _b = begin_use();
            assert_eq!(ACTIVE_JOBS.load(Ordering::SeqCst), before + 2);
        }
        assert_eq!(ACTIVE_JOBS.load(Ordering::SeqCst), before);
        assert!(idle_secs() < 5);
    }
}
