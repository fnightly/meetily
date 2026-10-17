/**
 * Pure helpers behind the window picker: filtering, labels and the compact value
 * shown on the home dashboard. Kept out of the component so they can be tested
 * without a DOM or a Tauri runtime.
 */

/** A top-level window returned by the `list_recordable_windows` command. */
export interface RecordableWindow {
  /** Raw window handle (Rust `isize`). */
  handle: number;
  /** Window title, which is what a person recognises it by. */
  title: string;
  /** Process that owns the window; capture follows the process, not the window. */
  process_id: number;
  /** Executable name, shown beside the title when the title is unhelpful. */
  display_name: string;
  /** Whether the owning process currently plays audio on the default output. */
  is_playing_audio: boolean;
}

function normalise(value: string | null | undefined): string {
  return (value ?? '').trim();
}

/** True when the title or the executable name contains `query`, case-insensitively. */
export function matchesWindowQuery(window: RecordableWindow, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return `${normalise(window.title)} ${normalise(window.display_name)}`.toLowerCase().includes(needle);
}

/**
 * Windows matching `query`, in the order the backend returned them.
 *
 * The backend already puts windows that are playing audio first and then sorts
 * alphabetically, so filtering must not re-order: that order is the useful one.
 */
export function filterWindows(
  windows: readonly RecordableWindow[],
  query: string,
): RecordableWindow[] {
  return windows.filter((window) => matchesWindowQuery(window, query));
}

/** The window title, falling back to the executable name when the title is empty. */
export function primaryWindowLabel(window: RecordableWindow): string {
  return normalise(window.title) || normalise(window.display_name);
}

/**
 * The executable name shown under the title. Empty when it would repeat the
 * primary label, so a row never reads "chrome.exe" twice.
 */
export function secondaryWindowLabel(window: RecordableWindow): string {
  const display = normalise(window.display_name);
  return display === primaryWindowLabel(window) ? '' : display;
}

/**
 * One-line value for the home setup grid: "Title · app.exe".
 *
 * `fallbackLabel` covers the whole-system choice (no window) and a window with
 * no usable title or executable, so the grid never renders an empty value.
 */
export function formatWindowValue(
  window: RecordableWindow | null,
  fallbackLabel: string,
): string {
  if (!window) return fallbackLabel;
  const primary = primaryWindowLabel(window);
  if (!primary) return fallbackLabel;
  const secondary = secondaryWindowLabel(window);
  return secondary ? `${primary} · ${secondary}` : primary;
}

/** True when both entries describe the same window (or both are null). */
export function isSameWindow(
  a: RecordableWindow | null,
  b: RecordableWindow | null,
): boolean {
  if (!a || !b) return a === b;
  return a.handle === b.handle && a.process_id === b.process_id;
}
