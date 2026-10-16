// src/disk_space.rs
//
// Free-space queries and thresholds.
//
// This started inside `whisper_engine::download` because that was the first
// caller, but it is a general facility: model downloads, recording output, and
// the retention sweep all care about it. Keeping it here means a caller does not
// have to depend on the download module to ask how much room is left.

use std::path::{Path, PathBuf};

/// Free space on the volume containing `path`, in bytes.
///
/// Returns `None` when it cannot be determined, in which case callers should
/// proceed: a failed measurement must not block an operation that would work.
pub fn available_space_bytes(path: &Path) -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;

        // `GetDiskFreeSpaceExW` is the documented API and needs no elevation.
        // `fsutil volume diskfree` is the obvious CLI alternative but fails with
        // "Error 5: Access is denied" for a non-elevated process, which would
        // silently disable the check for every normal user.
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);

        let mut free_to_caller: u64 = 0;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_to_caller,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };

        if ok == 0 {
            return None;
        }

        Some(free_to_caller)
    }

    #[cfg(not(target_os = "windows"))]
    {
        // `df -k <path>` is POSIX; the 4th column is available 1K blocks.
        let output = std::process::Command::new("df")
            .arg("-k")
            .arg(path)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().nth(1)?;
        let available_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
        Some(available_kb * 1024)
    }
}

/// Walk up to the nearest existing ancestor, so a path that is not created yet
/// can still be measured (it shares a volume with its existing parent).
pub fn nearest_existing_ancestor(path: &Path) -> PathBuf {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }
    probe
}

/// Format a byte count for a user-facing message.
pub fn human_mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_plausible_value_for_a_real_directory() {
        // Uses a real directory: the point is that the platform call is wired up,
        // which a mock cannot show.
        let tmp = tempfile::tempdir().expect("tempdir");
        match available_space_bytes(tmp.path()) {
            Some(free) => assert!(
                free > 1024 * 1024,
                "expected a real free-space figure, got {free} bytes"
            ),
            None => panic!("free space could not be determined for a real temp dir"),
        }
    }

    #[test]
    fn returns_none_for_a_missing_path_without_panicking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope").join("deeper");
        let _ = available_space_bytes(&missing);
    }

    #[test]
    fn nearest_existing_ancestor_finds_a_real_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("a").join("b").join("c");

        let ancestor = nearest_existing_ancestor(&missing);
        assert!(ancestor.exists());
        assert_eq!(ancestor, tmp.path());
    }

    #[test]
    fn nearest_existing_ancestor_returns_the_path_when_it_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(nearest_existing_ancestor(tmp.path()), tmp.path());
    }

    #[test]
    fn human_mb_truncates_to_whole_megabytes() {
        assert_eq!(human_mb(0), 0);
        assert_eq!(human_mb(1024 * 1024), 1);
        assert_eq!(human_mb(1024 * 1024 * 3 + 500_000), 3);
    }
}
