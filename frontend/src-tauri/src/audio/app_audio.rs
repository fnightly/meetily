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
// building: one application often plays through more than one process. WeChat, for
// instance, reports both `Weixin` and its mini-program runtime `WeChatAppEx`, and a
// picker listing those as separate entries would be unusable. Sessions are therefore
// folded up the process tree first, so each row is one thing a person recognises.

use std::collections::{HashMap, HashSet};

/// One application holding audio sessions on the default output device.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AudioApp {
    /// Every process backing this application. Process-loopback capture takes all of
    /// them, because the audio may come from a child of the process that owns a window.
    pub process_ids: Vec<u32>,
    /// Full path of the executable that names the group, when it could be read.
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

/// Walk up from `process_id` while each parent also holds an audio session.
///
/// Stopping at the first parent *without* a session is what keeps unrelated programs
/// apart: every process descends from `explorer.exe`, so following parents to the top
/// unconditionally would fold unrelated applications into one row.
fn group_root(process_id: u32, parent_of: &HashMap<u32, u32>, with_audio: &HashSet<u32>) -> u32 {
    let mut current = process_id;
    // A parent chain is short in practice; the cap only guards against a cycle in a
    // snapshot taken while processes were starting and exiting.
    let mut hops = 0;

    while hops < 32 {
        let Some(&parent) = parent_of.get(&current) else {
            break;
        };
        if parent == current || !with_audio.contains(&parent) {
            break;
        }
        current = parent;
        hops += 1;
    }

    current
}

/// Fold processes into one entry per application.
///
/// Split out from the COM calls so the grouping rule can be tested without audio
/// hardware: `processes` carries the process id, its executable and its label.
fn group_by_application(
    processes: Vec<(u32, Option<String>, String)>,
    parent_of: &HashMap<u32, u32>,
) -> Vec<AudioApp> {
    let with_audio: HashSet<u32> = processes.iter().map(|(pid, _, _)| *pid).collect();

    // Insertion order is kept so the outcome is stable for a given input.
    let mut order: Vec<u32> = Vec::new();
    let mut groups: HashMap<u32, AudioApp> = HashMap::new();

    for (process_id, executable, display_name) in processes {
        let root = group_root(process_id, parent_of, &with_audio);

        match groups.get_mut(&root) {
            Some(group) => {
                if !group.process_ids.contains(&process_id) {
                    group.process_ids.push(process_id);
                }
                // The root names the group; a child that happens to be seen first must
                // not decide what the row is called.
                if process_id == root {
                    group.executable = executable;
                    group.display_name = display_name;
                }
            }
            None => {
                order.push(root);
                groups.insert(
                    root,
                    AudioApp {
                        process_ids: vec![process_id],
                        executable,
                        display_name,
                    },
                );
            }
        }
    }

    let mut apps: Vec<AudioApp> = order
        .into_iter()
        .filter_map(|root| groups.remove(&root))
        .collect();

    for app in &mut apps {
        app.process_ids.sort_unstable();
        app.process_ids.dedup();
    }
    apps.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
    apps
}

#[cfg(target_os = "windows")]
pub use imp::list_audio_apps;

#[cfg(target_os = "windows")]
mod imp {
    use super::{display_name_from_path, group_by_application, AudioApp};
    use std::collections::HashMap;
    use windows::core::Interface;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
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

    /// Parent process id for every process, from one Toolhelp snapshot.
    ///
    /// A snapshot is far cheaper than asking per process, and paging the whole table at
    /// once avoids the gaps a per-process query would hit as processes exit.
    fn parent_map() -> HashMap<u32, u32> {
        let mut parents = HashMap::new();

        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return parents;
            };

            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };

            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }

            let _ = CloseHandle(snapshot);
        }

        parents
    }

    /// Every application with a session on the default output device.
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

            let mut seen: Vec<u32> = Vec::new();
            let mut processes: Vec<(u32, Option<String>, String)> = Vec::new();

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
                if process_id == 0 || seen.contains(&process_id) {
                    continue;
                }
                seen.push(process_id);

                let executable = executable_for(process_id);
                let display_name = executable
                    .as_deref()
                    .map(display_name_from_path)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Process {process_id}"));

                processes.push((process_id, executable, display_name));
            }

            Ok(group_by_application(processes, &parent_map()))
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

    fn process(pid: u32, name: &str) -> (u32, Option<String>, String) {
        (
            pid,
            Some(format!(r"C:\Program Files\{name}\{name}.exe")),
            name.to_string(),
        )
    }

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

    /// The case that motivated grouping: an application and its helper are one row.
    #[test]
    fn a_child_process_joins_its_parent() {
        let parents = HashMap::from([(200, 100), (100, 50)]);
        let apps = group_by_application(
            vec![process(100, "Weixin"), process(200, "WeChatAppEx")],
            &parents,
        );

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].display_name, "Weixin");
        assert_eq!(apps[0].process_ids, vec![100, 200]);
    }

    /// Every process descends from explorer, so a parent without audio must stop the
    /// walk or unrelated programs would collapse into one row.
    #[test]
    fn unrelated_processes_stay_separate() {
        let parents = HashMap::from([(100, 50), (300, 50)]);
        let apps = group_by_application(
            vec![process(100, "Weixin"), process(300, "msedge")],
            &parents,
        );

        assert_eq!(apps.len(), 2);
    }

    /// A parent holding audio but with a child that does not is still one row.
    #[test]
    fn a_process_without_audio_is_never_a_root() {
        let parents = HashMap::from([(200, 100)]);
        let apps = group_by_application(vec![process(200, "app")], &parents);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].display_name, "app");
        assert_eq!(apps[0].process_ids, vec![200]);
    }

    /// A cycle in the snapshot must not hang the walk. It should also not invent a
    /// relationship: two processes cannot really parent each other, so the only
    /// requirement is that the walk terminates and every process is still reported.
    #[test]
    fn a_parent_cycle_terminates() {
        let parents = HashMap::from([(100, 200), (200, 100)]);
        let apps = group_by_application(
            vec![process(100, "a"), process(200, "b")],
            &parents,
        );

        let mut reported: Vec<u32> = apps.iter().flat_map(|app| app.process_ids.clone()).collect();
        reported.sort_unstable();
        assert_eq!(reported, vec![100, 200], "both processes must still be listed");
    }

    /// The enumerator must not panic or error on a machine with no audio playing;
    /// an empty list is the expected answer there.
    #[test]
    fn listing_audio_apps_does_not_panic() {
        let apps = list_audio_apps().expect("listing audio apps should not fail");
        println!("audio apps: {apps:?}");
        for app in &apps {
            assert!(!app.process_ids.is_empty(), "every app needs a process");
            assert!(
                !app.process_ids.contains(&0),
                "process 0 should have been filtered out"
            );
            assert!(!app.display_name.is_empty(), "every app needs a label");
        }
    }
}
