// audio/app_audio.rs
//
// Which applications are currently producing audio on the default output device.
//
// This is the first half of per-app recording. cpal — which the rest of the capture
// path uses — can only open the whole-system mix, so "record just this meeting app"
// needs the WASAPI COM layer: the same layer plus a process id is what
// `ActivateAudioInterfaceAsync` turns into a process-scoped loopback stream.
//
// Kept separate from the capture work on purpose. Enumerating is read-only, it cannot
// disturb playback, and it answers the question that decides whether capture is worth
// building: meeting apps often play through an engine process whose name is not the
// one on the window, so the list a user would pick from has to be checkable first.

/// One application holding an audio session on the default output device.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AudioApp {
    /// Process id, which is what process-loopback capture will need.
    pub process_id: u32,
    /// Full path of the executable, when it could be read.
    pub executable: Option<String>,
    /// The file name without directory or extension, for display.
    pub display_name: String,
}

/// Turn a full executable path into something worth showing a person.
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

#[cfg(target_os = "windows")]
pub use imp::list_audio_apps;

#[cfg(target_os = "windows")]
mod imp {
    use super::{display_name_from_path, AudioApp};
    use windows::core::Interface;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Read the executable behind a process id.
    ///
    /// Uses `PROCESS_QUERY_LIMITED_INFORMATION` rather than full query rights: it is
    /// enough for the image name and is the only access an ordinary user process can
    /// get on another user's session, so the list does not silently depend on running
    /// elevated.
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
            let _ = CloseHandle(handle);

            if result.is_err() {
                return None;
            }

            Some(String::from_utf16_lossy(&buffer[..length as usize]))
        }
    }

    /// Every process with a session on the default output device.
    ///
    /// Sessions are per-process, not per-window: a meeting app that hosts several
    /// windows in one process appears once, and one that plays through a helper
    /// process appears under the helper's name.
    pub fn list_audio_apps() -> Result<Vec<AudioApp>, String> {
        unsafe {
            // Failing here means COM is already initialised with a different model on
            // this thread, which is fine for the calls below — only the first
            // initialisation's mode sticks.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("Could not create the audio device enumerator: {e}"))?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("No default output device: {e}"))?;

            let manager: IAudioSessionManager2 = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("Could not open the audio session manager: {e}"))?;

            let sessions = manager
                .GetSessionEnumerator()
                .map_err(|e| format!("Could not enumerate audio sessions: {e}"))?;

            let count = sessions
                .GetCount()
                .map_err(|e| format!("Could not count audio sessions: {e}"))?;

            let mut apps: Vec<AudioApp> = Vec::new();

            for index in 0..count {
                let Ok(control) = sessions.GetSession(index) else {
                    continue;
                };
                let Ok(control) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(process_id) = control.GetProcessId() else {
                    continue;
                };
                // The system sound session reports process 0; there is nothing to
                // attribute it to, and process-loopback cannot target it either.
                if process_id == 0 {
                    continue;
                }
                if apps.iter().any(|app| app.process_id == process_id) {
                    continue;
                }

                let executable = executable_for(process_id);
                let display_name = executable
                    .as_deref()
                    .map(display_name_from_path)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Process {process_id}"));

                apps.push(AudioApp {
                    process_id,
                    executable,
                    display_name,
                });
            }

            apps.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
            Ok(apps)
        }
    }
}

/// Unsupported platforms report an empty list rather than failing: nothing is
/// recording per-app there yet, and an error would look like a broken setting.
#[cfg(not(target_os = "windows"))]
pub fn list_audio_apps() -> Result<Vec<AudioApp>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_drops_directories_and_extension() {
        assert_eq!(
            display_name_from_path(r"C:\Program Files\Tencent\WeChat\WeChat.exe"),
            "WeChat"
        );
        assert_eq!(display_name_from_path("/usr/bin/obs"), "obs");
    }

    #[test]
    fn display_name_handles_upper_case_extension() {
        assert_eq!(display_name_from_path(r"D:\Apps\MSTeams.EXE"), "MSTeams");
    }

    #[test]
    fn display_name_survives_a_path_with_no_file_name() {
        assert_eq!(display_name_from_path(""), "");
    }

    /// The enumerator must not panic or error on a machine with no audio playing;
    /// an empty list is the expected answer there.
    #[test]
    fn listing_audio_apps_does_not_panic() {
        let apps = list_audio_apps().expect("listing audio apps should not fail");
        println!("audio apps: {apps:?}");
        for app in &apps {
            assert_ne!(app.process_id, 0, "process 0 should have been filtered out");
            assert!(!app.display_name.is_empty(), "every app needs a label");
        }
    }
}
