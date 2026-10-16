// audio/diarization_preferences.rs
//
// Tunables for speaker separation.
//
// The defaults are calibrated against the bundled embedding model, and for most meetings
// they should be left alone. They are exposed because they are the only lever when the
// result is wrong in a way the user can see — too many speakers means the clustering
// threshold is too low, too few means it is too high — and because replacing the embedding
// model invalidates the calibration entirely.
//
// Values are clamped rather than rejected. A threshold outside the usable band produces
// nonsense (everything in one cluster, or every segment its own speaker), and silently
// accepting it would look like the feature is broken.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use super::diarization_service::DiarizationConfig;
use super::speaker_cluster::CALIBRATED_THRESHOLD;

/// Store file holding the tunables.
const STORE_FILE: &str = "diarization_preferences.json";

/// Key within that store.
const STORE_KEY: &str = "preferences";

/// Lowest threshold accepted.
///
/// Below the calibrated value a speaker's own segments start landing in separate clusters,
/// so the result over-splits into people who were not there.
const MIN_THRESHOLD: f32 = 0.40;

/// Highest threshold accepted.
///
/// Approaching 1.0 makes every pair too far apart to merge, which collapses the meeting
/// into a single speaker.
const MAX_THRESHOLD: f32 = 0.95;

/// Lowest speaker bound accepted.
const MIN_SPEAKERS: usize = 2;

/// Highest speaker bound accepted.
///
/// A bound is a ceiling, not a target, so a high value is harmless; this only keeps a
/// typo from being stored.
const MAX_SPEAKERS: usize = 32;

/// Tunables for speaker separation, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiarizationPreferences {
    /// Clustering threshold: higher merges more, producing fewer speakers.
    pub threshold: f32,
    /// Upper bound on speakers.
    pub max_speakers: usize,
}

impl Default for DiarizationPreferences {
    fn default() -> Self {
        let config = DiarizationConfig::default();
        Self {
            threshold: config.threshold,
            max_speakers: config.max_speakers,
        }
    }
}

impl DiarizationPreferences {
    /// Clamp every field into the usable band.
    ///
    /// A non-finite threshold is replaced by the calibrated default rather than clamped:
    /// `f32::clamp` propagates NaN, and a stored NaN would make every comparison in the
    /// clustering false, so nothing would ever merge.
    pub fn sanitised(self) -> Self {
        let threshold = if self.threshold.is_finite() {
            self.threshold.clamp(MIN_THRESHOLD, MAX_THRESHOLD)
        } else {
            CALIBRATED_THRESHOLD
        };

        Self {
            threshold,
            max_speakers: self.max_speakers.clamp(MIN_SPEAKERS, MAX_SPEAKERS),
        }
    }

    /// Whether these are the calibrated defaults.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Convert to the configuration the service takes.
    pub fn to_config(self) -> DiarizationConfig {
        let sane = self.sanitised();
        DiarizationConfig {
            threshold: sane.threshold,
            max_speakers: sane.max_speakers,
        }
    }
}

/// The bands the interface should present, so the limits are stated once.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DiarizationPreferenceBounds {
    pub min_threshold: f32,
    pub max_threshold: f32,
    pub min_speakers: usize,
    pub max_speakers: usize,
    /// The calibrated threshold, for a "reset" action to restore.
    pub calibrated_threshold: f32,
}

impl DiarizationPreferenceBounds {
    pub fn current() -> Self {
        Self {
            min_threshold: MIN_THRESHOLD,
            max_threshold: MAX_THRESHOLD,
            min_speakers: MIN_SPEAKERS,
            max_speakers: MAX_SPEAKERS,
            calibrated_threshold: CALIBRATED_THRESHOLD,
        }
    }
}

/// Load the stored tunables, falling back to the calibrated defaults.
///
/// Never fails: a missing, unreadable or malformed store means the caller wants the
/// defaults, and making separation fail because a preference file is corrupt would be a
/// worse outcome than ignoring it.
pub fn load_diarization_preferences<R: Runtime>(app: &AppHandle<R>) -> DiarizationPreferences {
    let store = match app.store(STORE_FILE) {
        Ok(store) => store,
        Err(error) => {
            log::warn!("Could not open the diarization preference store ({error}); using defaults");
            return DiarizationPreferences::default();
        }
    };

    let stored: Option<DiarizationPreferences> = store
        .get(STORE_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok());

    match stored {
        Some(preferences) => {
            let sane = preferences.sanitised();
            if sane != preferences {
                log::warn!("Stored diarization preferences were out of range and have been clamped");
            }
            sane
        }
        None => DiarizationPreferences::default(),
    }
}

/// Persist the tunables, clamping them first.
///
/// Returns the stored value so the caller can show what was actually saved rather than
/// what was asked for.
pub fn save_diarization_preferences<R: Runtime>(
    app: &AppHandle<R>,
    preferences: DiarizationPreferences,
) -> Result<DiarizationPreferences, String> {
    let sane = preferences.sanitised();

    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access the diarization preference store: {e}"))?;

    let value = serde_json::to_value(sane)
        .map_err(|e| format!("Failed to serialize diarization preferences: {e}"))?;

    store.set(STORE_KEY, value);
    store
        .save()
        .map_err(|e| format!("Failed to save diarization preferences: {e}"))?;

    log::info!(
        "Diarization preferences saved: threshold={}, max_speakers={}",
        sane.threshold,
        sane.max_speakers
    );

    Ok(sane)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_calibrated_ones() {
        // A default that drifted from the calibration would silently change every result.
        let default = DiarizationPreferences::default();

        assert_eq!(default.threshold, CALIBRATED_THRESHOLD);
        assert_eq!(default.max_speakers, DiarizationConfig::default().max_speakers);
        assert_eq!(default, default.sanitised());
    }

    #[test]
    fn is_default_recognises_the_defaults() {
        assert!(DiarizationPreferences::default().is_default());
        assert!(!DiarizationPreferences { threshold: 0.6, ..Default::default() }.is_default());
    }

    #[test]
    fn a_threshold_below_the_band_is_raised_to_the_floor() {
        // Accepted as-is it would split one speaker's own segments apart.
        let clamped = DiarizationPreferences { threshold: 0.05, max_speakers: 12 }.sanitised();
        assert_eq!(clamped.threshold, MIN_THRESHOLD);
    }

    #[test]
    fn a_threshold_above_the_band_is_lowered_to_the_ceiling() {
        // Accepted as-is nothing would merge and the meeting would become one speaker.
        let clamped = DiarizationPreferences { threshold: 0.999, max_speakers: 12 }.sanitised();
        assert_eq!(clamped.threshold, MAX_THRESHOLD);
    }

    #[test]
    fn a_non_finite_threshold_falls_back_to_the_calibration() {
        // `clamp` propagates NaN, and a stored NaN makes every comparison false, so nothing
        // would ever merge.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let clamped = DiarizationPreferences { threshold: bad, max_speakers: 12 }.sanitised();
            assert_eq!(
                clamped.threshold, CALIBRATED_THRESHOLD,
                "{bad} should fall back to the calibrated threshold"
            );
        }
    }

    #[test]
    fn a_speaker_bound_below_the_floor_is_raised() {
        // A bound of zero or one makes the ceiling meaningless and merges everyone.
        for bad in [0, 1] {
            assert_eq!(
                DiarizationPreferences { threshold: CALIBRATED_THRESHOLD, max_speakers: bad }
                    .sanitised()
                    .max_speakers,
                MIN_SPEAKERS
            );
        }
    }

    #[test]
    fn a_speaker_bound_above_the_ceiling_is_lowered() {
        // A bound is a ceiling, so a huge value is harmless in effect, but a typo should not
        // be stored as if it meant something.
        assert_eq!(
            DiarizationPreferences { threshold: CALIBRATED_THRESHOLD, max_speakers: 5_000 }
                .sanitised()
                .max_speakers,
            MAX_SPEAKERS
        );
    }

    #[test]
    fn a_value_inside_the_band_is_left_alone() {
        let asked = DiarizationPreferences { threshold: 0.66, max_speakers: 6 };
        assert_eq!(asked.sanitised(), asked);
    }

    #[test]
    fn sanitising_is_idempotent() {
        // Clamping is applied on both load and save, so applying it twice must not drift.
        let once = DiarizationPreferences { threshold: 0.123, max_speakers: 99 }.sanitised();
        assert_eq!(once.sanitised(), once);
    }

    #[test]
    fn to_config_carries_the_sanitised_values() {
        let config = DiarizationPreferences { threshold: 0.01, max_speakers: 0 }.to_config();

        assert_eq!(config.threshold, MIN_THRESHOLD);
        assert_eq!(config.max_speakers, MIN_SPEAKERS);
    }

    #[test]
    fn the_defaults_convert_to_the_service_defaults() {
        // The stored defaults and the service defaults must agree, or separation would
        // change the moment a preference file is written.
        let from_preferences = DiarizationPreferences::default().to_config();
        let from_service = DiarizationConfig::default();

        assert_eq!(from_preferences.threshold, from_service.threshold);
        assert_eq!(from_preferences.max_speakers, from_service.max_speakers);
    }

    #[test]
    fn bounds_state_the_bands_and_the_calibration() {
        let bounds = DiarizationPreferenceBounds::current();

        assert!(bounds.min_threshold < bounds.calibrated_threshold);
        assert!(bounds.calibrated_threshold < bounds.max_threshold);
        assert!(bounds.min_speakers < bounds.max_speakers);
        assert_eq!(bounds.calibrated_threshold, CALIBRATED_THRESHOLD);
    }

    #[test]
    fn preferences_survive_a_json_round_trip() {
        let preferences = DiarizationPreferences { threshold: 0.68, max_speakers: 8 };
        let json = serde_json::to_string(&preferences).expect("serialise");
        let parsed: DiarizationPreferences = serde_json::from_str(&json).expect("deserialise");

        assert_eq!(parsed, preferences);
    }

    #[test]
    fn a_partial_document_is_rejected_rather_than_defaulted_per_field() {
        // Defaulting one missing field would pair a chosen threshold with an unintended
        // bound, so an incomplete document falls back to the defaults as a whole.
        let parsed: Result<DiarizationPreferences, _> = serde_json::from_str(r#"{"threshold":0.8}"#);
        assert!(parsed.is_err());
    }
}
