// whisper_engine/download.rs
//
// Download-time pre-flight checks.
//
// Kept separate from `whisper_engine.rs` so the disk-space arithmetic can be
// unit tested without building a `WhisperEngine` (which requires a loaded model
// context and a Tauri runtime).

use std::path::Path;

/// Rough safety margin added on top of the model size.
///
/// The published catalog sizes are rounded down, and the filesystem needs room
/// for its own metadata, so a download that only *just* fits is not worth
/// starting.
const DISK_SPACE_HEADROOM_BYTES: u64 = 64 * 1024 * 1024;

/// Known size of a Whisper model in bytes, from the shared catalog.
///
/// Returns `None` for an unknown model name; callers should then skip the check
/// rather than guess.
pub fn known_model_size_bytes(model_name: &str) -> Option<u64> {
    crate::config::WHISPER_MODEL_CATALOG
        .iter()
        .find(|(name, ..)| *name == model_name)
        .map(|(_, _, size_mb, ..)| u64::from(*size_mb) * 1024 * 1024)
}

/// Whether `free_bytes` can hold `needed_bytes` plus headroom.
///
/// Subtracts rather than adds: `needed + headroom` can overflow, and a
/// saturating add silently caps at `u64::MAX`, which makes the comparison pass
/// for absurd inputs and reports room that does not exist.
///
/// Pure so the boundary cases are testable without touching a real disk.
pub fn has_room_for(free_bytes: u64, needed_bytes: u64) -> bool {
    if free_bytes < needed_bytes {
        return false;
    }
    free_bytes - needed_bytes >= DISK_SPACE_HEADROOM_BYTES
}

/// Fail with an actionable message when `models_dir` cannot hold `model_name`.
///
/// A truncated model file is far harder to diagnose than a refusal, so this runs
/// before any bytes are written.
pub fn ensure_space_for_model(models_dir: &Path, model_name: &str) -> anyhow::Result<()> {
    let Some(needed) = known_model_size_bytes(model_name) else {
        return Ok(());
    };

    // The directory may not exist yet; walk up to the nearest existing ancestor,
    // which is on the same volume in every realistic layout.
    let probe = crate::disk_space::nearest_existing_ancestor(models_dir);

    let Some(free) = crate::disk_space::available_space_bytes(&probe) else {
        log::warn!(
            "Could not determine free space at {}; skipping the disk-space check",
            probe.display()
        );
        return Ok(());
    };

    if has_room_for(free, needed) {
        return Ok(());
    }

    let mib = 1024 * 1024;
    anyhow::bail!(
        "Not enough disk space to download '{}': needs about {} MB (plus headroom), \
         but only {} MB is free at {}. Free up space or point the models folder at a \
         drive with more room (Settings → Data Storage Locations), then try again.",
        model_name,
        needed / mib,
        free / mib,
        probe.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_room_requires_size_plus_headroom() {
        let needed = 100 * 1024 * 1024;
        assert!(has_room_for(needed + DISK_SPACE_HEADROOM_BYTES, needed));
        // Exactly the file size is not enough: headroom must remain.
        assert!(!has_room_for(needed, needed));
        assert!(!has_room_for(needed - 1, needed));
    }

    #[test]
    fn has_room_is_false_for_an_empty_disk() {
        assert!(!has_room_for(0, 1024));
    }

    #[test]
    fn has_room_does_not_overflow_on_absurd_sizes() {
        // A saturating add must not wrap into "there is room".
        assert!(!has_room_for(u64::MAX, u64::MAX));
    }

    #[test]
    fn known_size_covers_every_catalog_model() {
        for (name, ..) in crate::config::WHISPER_MODEL_CATALOG {
            assert!(
                known_model_size_bytes(name).is_some(),
                "catalog entry {name} has no resolvable size"
            );
        }
    }

    #[test]
    fn known_size_is_none_for_unknown_model() {
        assert!(known_model_size_bytes("definitely-not-a-model").is_none());
    }

    #[test]
    fn known_size_matches_catalog_megabytes() {
        // `base` is catalogued at 142 MB.
        assert_eq!(known_model_size_bytes("base"), Some(142 * 1024 * 1024));
    }

    #[test]
    fn unknown_model_skips_the_check_rather_than_failing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // An unknown model must not be refused just because its size is unknown.
        assert!(ensure_space_for_model(tmp.path(), "no-such-model").is_ok());
    }

    #[test]
    fn ensure_space_passes_for_a_small_model_on_a_real_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // `tiny-q5_1` is 31 MB; any machine able to run this test has that free.
        assert!(ensure_space_for_model(tmp.path(), "tiny-q5_1").is_ok());
    }
}
