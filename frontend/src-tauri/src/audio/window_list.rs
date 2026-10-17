// audio/window_list.rs
//
// The windows a user can point a recording at.
//
// WASAPI has no window-scoped capture: the narrowest unit it offers is a process, and
// that is what `process_capture` opens. A window is still the right thing to ask a person
// to pick, though — "record that call" means the window they are looking at, not a
// process id — so this module resolves a window back to the process that owns it and
// reports which of those processes are actually producing sound.
//
// A window whose process has several windows therefore captures all of them. That is a
// property of the platform, not an omission, and the interface should say so rather than
// promise a precision that does not exist.

/// A top-level window a recording could be pointed at.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RecordableWindow {
    /// Raw window handle, as `isize` so it survives the trip to the renderer.
    pub handle: isize,
    /// Window title, which is what a person recognises it by.
    pub title: String,
    /// Process that owns the window. This is what capture is actually opened against.
    pub process_id: u32,
    /// Executable name, shown beside the title when the title is unhelpful.
    pub display_name: String,
    /// Whether this process currently has an audio session on the default output.
    ///
    /// The useful windows almost always have this, so callers can sort them first.
    pub is_playing_audio: bool,
}

#[cfg(target_os = "windows")]
pub use imp::list_recordable_windows;

#[cfg(target_os = "windows")]
mod imp {
    use super::RecordableWindow;
    use std::collections::HashSet;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };

    /// Read the executable behind a process id.
    fn executable_for(process_id: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(buffer.as_mut_ptr()),
                &mut length,
            );
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            if result.is_err() {
                return None;
            }
            Some(String::from_utf16_lossy(&buffer[..length as usize]))
        }
    }

    /// Executable file name without directory or extension.
    fn display_name_from_path(path: &str) -> String {
        let file = path
            .rsplit(['\\', '/'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(path);
        file.strip_suffix(".exe")
            .or_else(|| file.strip_suffix(".EXE"))
            .unwrap_or(file)
            .to_string()
    }

    /// Collect windows into the vector behind `lparam`.
    unsafe extern "system" fn collect_window(window: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<(HWND, String, u32)>) };

        if unsafe { IsWindowVisible(window) }.as_bool() == false {
            return BOOL(1);
        }

        // Tool windows are the invisible scaffolding applications put behind themselves —
        // tray popups, IME composition, invisible message-only hosts. They carry no title
        // a person would recognise, and listing them is noise.
        let ex_style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return BOOL(1);
        }

        let length = unsafe { GetWindowTextLengthW(window) };
        if length <= 0 {
            return BOOL(1);
        }

        let mut buffer = vec![0u16; length as usize + 1];
        let copied = unsafe { GetWindowTextW(window, &mut buffer) };
        if copied <= 0 {
            return BOOL(1);
        }
        buffer.truncate(copied as usize);
        let title = String::from_utf16_lossy(&buffer);

        let mut process_id = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
        if process_id == 0 {
            return BOOL(1);
        }

        out.push((window, title, process_id));
        BOOL(1)
    }

    /// Every window worth offering, with the ones producing audio first.
    pub fn list_recordable_windows() -> Result<Vec<RecordableWindow>, String> {
        let mut collected: Vec<(HWND, String, u32)> = Vec::new();

        unsafe {
            EnumWindows(
                Some(collect_window),
                LPARAM(&mut collected as *mut _ as isize),
            )
            .map_err(|e| format!("could not enumerate windows: {e}"))?;
        }

        // Which processes are currently producing audio. This is the same source the
        // application picker uses, so both lists agree about what is "live".
        let playing: HashSet<u32> = crate::audio::app_audio::audio_related_processes();

        // The app's own windows are not useful targets: recording yourself captures
        // nothing but the interface's own sounds.
        let own_process = std::process::id();

        let mut windows: Vec<RecordableWindow> = collected
            .into_iter()
            .filter(|(_, _, pid)| *pid != own_process)
            .map(|(window, title, process_id)| {
                let executable = executable_for(process_id);
                let display_name = executable
                    .as_deref()
                    .map(display_name_from_path)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Process {process_id}"));

                RecordableWindow {
                    handle: window.0 as isize,
                    title,
                    process_id,
                    display_name,
                    is_playing_audio: playing.contains(&process_id),
                }
            })
            .collect();

        // Playing windows first, then alphabetical, so the likely target is at the top.
        windows.sort_by(|a, b| {
            b.is_playing_audio
                .cmp(&a.is_playing_audio)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });

        Ok(windows)
    }
}

/// Unsupported platforms have no windows to offer.
#[cfg(not(target_os = "windows"))]
pub fn list_recordable_windows() -> Result<Vec<RecordableWindow>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Must not crash or fail on a machine with no windows, and every entry must be
    /// usable as a recording target.
    #[test]
    fn listing_windows_returns_usable_entries() {
        let windows = list_recordable_windows().expect("listing windows should not fail");
        println!("windows: {}", windows.len());
        for window in windows.iter().take(5) {
            println!(
                "  {} ({}) playing={}",
                window.title, window.display_name, window.is_playing_audio
            );
        }

        for window in &windows {
            assert!(!window.title.is_empty(), "a window with no title is not pickable");
            assert_ne!(window.process_id, 0, "a window must belong to a process");
            assert_ne!(window.handle, 0, "a window needs a handle to identify it");
        }

        // Playing windows sort ahead of the rest.
        let mut seen_quiet = false;
        for window in &windows {
            if !window.is_playing_audio {
                seen_quiet = true;
            } else {
                assert!(!seen_quiet, "a playing window appeared after a quiet one");
            }
        }
    }
}
