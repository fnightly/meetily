// audio/retention.rs
//
// Retention policy for saved recordings.
//
// Recordings accumulate: a one-hour WAV is hundreds of megabytes, and the app
// has already hit a full disk once, which broke both building and downloading.
// OnlyR solves this with a recordings lifetime; this is the same idea, but the
// decision logic is a pure function so the rules can be tested without touching
// a filesystem.
//
// Deleting a user's audio is destructive, so the rules are deliberately
// conservative:
//   - disabled unless a lifetime is configured;
//   - a recording must be older than the lifetime *and* finished;
//   - an in-progress recording is never a candidate;
//   - unknown ages are kept, not deleted.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A saved recording folder considered for removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingCandidate {
    pub path: PathBuf,
    /// When the recording finished. `None` means unknown.
    pub completed_at: Option<SystemTime>,
}

impl RecordingCandidate {
    pub fn new(path: impl Into<PathBuf>, completed_at: Option<SystemTime>) -> Self {
        Self {
            path: path.into(),
            completed_at,
        }
    }
}

/// Why a candidate was kept, for logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepReason {
    /// Retention is disabled (lifetime of zero).
    RetentionDisabled,
    /// Still within the retention window.
    WithinLifetime,
    /// Completion time is unknown, so its age cannot be judged.
    UnknownAge,
    /// Completion time is in the future (clock skew); treat as recent.
    NotYetComplete,
    /// The path does not exist any more.
    Missing,
}

/// Split candidates into what to delete and what to keep.
///
/// `lifetime_days` of zero disables deletion entirely, which is the default:
/// nothing is removed unless the user opted in.
pub fn select_expired(
    candidates: &[RecordingCandidate],
    lifetime_days: u32,
    now: SystemTime,
) -> (Vec<PathBuf>, Vec<(PathBuf, KeepReason)>) {
    if lifetime_days == 0 {
        return (
            Vec::new(),
            candidates
                .iter()
                .map(|c| (c.path.clone(), KeepReason::RetentionDisabled))
                .collect(),
        );
    }

    let lifetime = Duration::from_secs(u64::from(lifetime_days) * 24 * 60 * 60);
    let mut expired = Vec::new();
    let mut kept = Vec::new();

    for candidate in candidates {
        if !candidate.path.exists() {
            kept.push((candidate.path.clone(), KeepReason::Missing));
            continue;
        }

        let Some(completed_at) = candidate.completed_at else {
            // An unknown age must not be treated as "infinitely old"; that would
            // delete a recording the app simply failed to date.
            kept.push((candidate.path.clone(), KeepReason::UnknownAge));
            continue;
        };

        // `duration_since` fails when `completed_at` is in the future, which means
        // a clock change. Such a recording is recent by definition.
        let Ok(age) = now.duration_since(completed_at) else {
            kept.push((candidate.path.clone(), KeepReason::NotYetComplete));
            continue;
        };

        if age >= lifetime {
            expired.push(candidate.path.clone());
        } else {
            kept.push((candidate.path.clone(), KeepReason::WithinLifetime));
        }
    }

    (expired, kept)
}

/// Whether a folder is a recorded-meeting folder.
///
/// Meetings are written as `<recordings root>/<name>/<audio file>` plus a
/// `metadata.json`; checkpoint directories live inside them. Anything lacking the
/// metadata marker is not ours to delete.
pub fn is_meeting_folder(path: &Path) -> bool {
    path.is_dir() && path.join("metadata.json").is_file()
}

/// Free-space check thresholds, resolved from preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskWatch {
    /// Warn below this many MB. Zero disables the warning.
    pub warn_below_mb: u32,
}

impl DiskWatch {
    pub fn new(warn_below_mb: u32) -> Self {
        Self { warn_below_mb }
    }

    pub fn is_enabled(&self) -> bool {
        self.warn_below_mb > 0
    }

    pub fn threshold_bytes(&self) -> u64 {
        u64::from(self.warn_below_mb) * 1024 * 1024
    }

    /// Whether `free_bytes` is low enough to warn about.
    pub fn is_low(&self, free_bytes: u64) -> bool {
        self.is_enabled() && free_bytes < self.threshold_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days_ago(days: u64, now: SystemTime) -> SystemTime {
        now - Duration::from_secs(days * 24 * 60 * 60)
    }

    /// Candidates whose paths exist, in a temp dir.
    fn existing_candidates(ages: &[u64], now: SystemTime) -> (tempfile::TempDir, Vec<RecordingCandidate>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let candidates = ages
            .iter()
            .enumerate()
            .map(|(i, days)| {
                let path = dir.path().join(format!("meeting-{i}"));
                std::fs::create_dir_all(&path).expect("create meeting dir");
                RecordingCandidate::new(path, Some(days_ago(*days, now)))
            })
            .collect();
        (dir, candidates)
    }

    #[test]
    fn retention_disabled_keeps_everything() {
        let now = SystemTime::now();
        let (_dir, candidates) = existing_candidates(&[1, 100, 400], now);

        let (expired, kept) = select_expired(&candidates, 0, now);

        assert!(expired.is_empty(), "nothing may be deleted when disabled");
        assert_eq!(kept.len(), 3);
        assert!(kept.iter().all(|(_, r)| *r == KeepReason::RetentionDisabled));
    }

    #[test]
    fn deletes_only_recordings_older_than_the_lifetime() {
        let now = SystemTime::now();
        let (_dir, candidates) = existing_candidates(&[1, 29, 31, 90], now);

        let (expired, kept) = select_expired(&candidates, 30, now);

        assert_eq!(expired.len(), 2, "31 and 90 days old should expire");
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|(_, r)| *r == KeepReason::WithinLifetime));
    }

    #[test]
    fn a_recording_exactly_at_the_lifetime_expires() {
        let now = SystemTime::now();
        let (_dir, candidates) = existing_candidates(&[30], now);

        let (expired, _) = select_expired(&candidates, 30, now);
        assert_eq!(expired.len(), 1);
    }

    #[test]
    fn unknown_completion_time_is_kept() {
        let now = SystemTime::now();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("undated");
        std::fs::create_dir_all(&path).expect("create");

        let candidates = vec![RecordingCandidate::new(path, None)];
        let (expired, kept) = select_expired(&candidates, 1, now);

        // Deleting a recording simply because the app could not date it would be
        // data loss caused by a metadata problem.
        assert!(expired.is_empty());
        assert_eq!(kept[0].1, KeepReason::UnknownAge);
    }

    #[test]
    fn future_completion_time_is_treated_as_recent() {
        let now = SystemTime::now();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("from-the-future");
        std::fs::create_dir_all(&path).expect("create");

        let candidates = vec![RecordingCandidate::new(path, Some(now + Duration::from_secs(86_400)))];
        let (expired, kept) = select_expired(&candidates, 1, now);

        assert!(expired.is_empty());
        assert_eq!(kept[0].1, KeepReason::NotYetComplete);
    }

    #[test]
    fn missing_paths_are_reported_not_deleted() {
        let now = SystemTime::now();
        let candidates = vec![RecordingCandidate::new(
            PathBuf::from("Z:\\definitely\\not\\here"),
            Some(days_ago(400, now)),
        )];

        let (expired, kept) = select_expired(&candidates, 1, now);
        assert!(expired.is_empty());
        assert_eq!(kept[0].1, KeepReason::Missing);
    }

    #[test]
    fn meeting_folder_requires_the_metadata_marker() {
        let dir = tempfile::tempdir().expect("tempdir");

        let not_ours = dir.path().join("random");
        std::fs::create_dir_all(&not_ours).expect("create");
        assert!(!is_meeting_folder(&not_ours));

        let ours = dir.path().join("Team Standup");
        std::fs::create_dir_all(&ours).expect("create");
        std::fs::write(ours.join("metadata.json"), b"{}").expect("write");
        assert!(is_meeting_folder(&ours));
    }

    #[test]
    fn disk_watch_is_disabled_by_zero() {
        let watch = DiskWatch::new(0);
        assert!(!watch.is_enabled());
        assert!(!watch.is_low(0), "a disabled watch must never warn");
    }

    #[test]
    fn disk_watch_warns_below_the_threshold() {
        let watch = DiskWatch::new(1024); // 1 GB

        assert!(watch.is_low(0));
        assert!(watch.is_low(512 * 1024 * 1024));
        assert!(!watch.is_low(1024 * 1024 * 1024), "at the threshold is not low");
        assert!(!watch.is_low(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn disk_watch_threshold_matches_megabytes() {
        assert_eq!(DiskWatch::new(100).threshold_bytes(), 100 * 1024 * 1024);
    }
}
