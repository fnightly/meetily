// recording_bar.rs
//
// Floating recording bar: a small always-on-top window that shows the recording
// state while the main window is not in front, so the user can see that the
// meeting is still being recorded and pause or stop it without switching back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Listener, LogicalPosition, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "recording-bar";

const BAR_WIDTH: f64 = 320.0;
const BAR_HEIGHT: f64 = 56.0;
const BAR_TOP_MARGIN: f64 = 48.0;

/// Whether the main window currently has focus. Starts true: the app opens in front.
static MAIN_FOCUSED: AtomicBool = AtomicBool::new(true);

/// Serialises window creation so two quick syncs cannot both build it.
static WINDOW_LOCK: Mutex<()> = Mutex::new(());

/// Keep the bar in step with the recording lifecycle. Called once from `setup`.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    for event in ["recording-started", "recording-stopped"] {
        let handle = app.clone();
        app.listen(event, move |_| sync(&handle));
    }
}

/// Called from the main window's focus events.
pub fn on_main_focus_changed<R: Runtime>(app: &AppHandle<R>, focused: bool) {
    MAIN_FOCUSED.store(focused, Ordering::SeqCst);
    sync(app);
}

/// Show the bar while a recording is live and the main window is not focused;
/// hide it otherwise.
///
/// Runs off the calling thread: window events arrive on the main thread, and
/// building a webview there deadlocks on Windows (WebView2).
pub fn sync<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Let a focus change that immediately follows another (e.g. clicking the
        // bar's "open" button) settle before deciding, so the bar does not flicker.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        apply(&app);
    });
}

fn apply<R: Runtime>(app: &AppHandle<R>) {
    let recording = crate::audio::recording_commands::is_recording_now();
    let should_show = recording && !MAIN_FOCUSED.load(Ordering::SeqCst);

    let _guard = WINDOW_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(window) = app.get_webview_window(LABEL) {
        let visible = window.is_visible().unwrap_or(false);
        if should_show && !visible {
            position_bar(app, &window);
            if let Err(e) = window.show() {
                log::error!("Failed to show recording bar: {}", e);
            }
        } else if !should_show && visible {
            if let Err(e) = window.hide() {
                log::error!("Failed to hide recording bar: {}", e);
            }
        }
        return;
    }

    // Built on first need rather than at startup, so a user who never records
    // with the app in the background never pays for a second webview.
    if !should_show {
        return;
    }

    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("recording-bar".into()))
        .title("Meetily recording")
        .inner_size(BAR_WIDTH, BAR_HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false);

    match builder.build() {
        Ok(window) => {
            position_bar(app, &window);
            if let Err(e) = window.show() {
                log::error!("Failed to show recording bar: {}", e);
            }
            log::info!("Recording bar created");
        }
        Err(e) => log::error!("Failed to create recording bar: {}", e),
    }
}

/// Top centre of the monitor the main window is on (or the primary one).
fn position_bar<R: Runtime>(app: &AppHandle<R>, window: &tauri::WebviewWindow<R>) {
    let monitor = app
        .get_webview_window("main")
        .and_then(|main| main.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());

    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let origin = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        let x = origin.x + (size.width - BAR_WIDTH) / 2.0;
        let y = origin.y + BAR_TOP_MARGIN;
        let _ = window.set_position(LogicalPosition::new(x, y));
    }
}

/// Stop the recording from the bar. The main window's post-processing picks
/// it up through `recording-stop-complete`, as for the tray.
#[tauri::command]
pub fn recording_bar_stop<R: Runtime>(app: AppHandle<R>) {
    crate::tray::request_stop_recording(&app);
}

/// Bring the main window to the front (which hides the bar).
#[tauri::command]
pub fn recording_bar_open_main<R: Runtime>(app: AppHandle<R>) {
    crate::tray::focus_main_window(&app);
}
