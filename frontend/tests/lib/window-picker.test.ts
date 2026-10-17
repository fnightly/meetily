import { describe, expect, test } from 'bun:test';
import {
  filterWindows,
  formatWindowValue,
  isSameWindow,
  matchesWindowQuery,
  primaryWindowLabel,
  secondaryWindowLabel,
  type RecordableWindow,
} from '../../src/lib/window-picker';

/**
 * The window picker's decisions live in a pure module: which rows survive the
 * search box, what each row is labelled, and what the home grid shows once a
 * window (or the whole system) is selected. These checks read that logic
 * directly, without rendering a dialog or needing Tauri.
 */

const window = (over: Partial<RecordableWindow> = {}): RecordableWindow => ({
  handle: 1,
  title: 'Weekly sync - Chrome',
  process_id: 100,
  display_name: 'chrome.exe',
  is_playing_audio: false,
  ...over,
});

describe('filterWindows', () => {
  test('an empty query keeps every window, in backend order', () => {
    const playing = window({ handle: 1, title: 'Call - Zoom', display_name: 'zoom.exe', is_playing_audio: true });
    const silent = window({ handle: 2, title: 'Docs - Chrome', display_name: 'chrome.exe' });

    // The backend already orders playing windows first; filtering must preserve it.
    expect(filterWindows([playing, silent], '')).toEqual([playing, silent]);
    expect(filterWindows([playing, silent], '   ')).toEqual([playing, silent]);
  });

  test('matches the window title case-insensitively', () => {
    const target = window({ title: 'Team Standup - Google Meet' });
    const other = window({ handle: 2, title: 'Notepad', display_name: 'notepad.exe' });

    expect(filterWindows([target, other], 'standup')).toEqual([target]);
    expect(filterWindows([target, other], 'TEAM')).toEqual([target]);
  });

  test('matches the executable name when the title says nothing useful', () => {
    const target = window({ title: 'Untitled', display_name: 'Spotify.exe' });

    expect(filterWindows([target], 'spotify')).toEqual([target]);
    expect(filterWindows([target], 'SPOTIFY.EXE')).toEqual([target]);
  });

  test('returns nothing when no window matches', () => {
    expect(filterWindows([window()], 'obs')).toEqual([]);
  });

  test('does not mutate the caller\'s list', () => {
    const windows = [window({ handle: 1 }), window({ handle: 2, title: 'Music - Spotify' })];
    const snapshot = [...windows];

    filterWindows(windows, 'spotify');

    expect(windows).toEqual(snapshot);
  });

  test('can be asked about a single window', () => {
    expect(matchesWindowQuery(window(), 'weekly')).toBe(true);
    expect(matchesWindowQuery(window(), 'absent')).toBe(false);
    expect(matchesWindowQuery(window(), '')).toBe(true);
  });
});

describe('window labels', () => {
  test('uses the window title as the primary label', () => {
    expect(primaryWindowLabel(window())).toBe('Weekly sync - Chrome');
  });

  test('falls back to the executable when the title is blank', () => {
    expect(primaryWindowLabel(window({ title: '' }))).toBe('chrome.exe');
    expect(primaryWindowLabel(window({ title: '   ' }))).toBe('chrome.exe');
  });

  test('shows the executable as secondary text when it adds information', () => {
    expect(secondaryWindowLabel(window())).toBe('chrome.exe');
  });

  test('does not repeat the executable when it is already the primary label', () => {
    const windowWithoutTitle = window({ title: '', display_name: 'chrome.exe' });
    expect(primaryWindowLabel(windowWithoutTitle)).toBe('chrome.exe');
    expect(secondaryWindowLabel(windowWithoutTitle)).toBe('');
  });
});

describe('formatWindowValue', () => {
  test('the whole-system choice renders the caller-supplied fallback label', () => {
    // null means "record the whole system mix", so the home grid keeps the
    // existing default label rather than inventing one here.
    expect(formatWindowValue(null, 'System default')).toBe('System default');
  });

  test('joins the window title and app name on one line', () => {
    expect(formatWindowValue(window(), 'System default')).toBe('Weekly sync - Chrome · chrome.exe');
  });

  test('does not leave a dangling separator when there is no app name', () => {
    expect(formatWindowValue(window({ title: 'Meeting', display_name: 'Meeting' }), 'System default'))
      .toBe('Meeting');
  });

  test('falls back when the window has neither a title nor an executable', () => {
    expect(formatWindowValue(window({ title: '', display_name: '' }), 'Application window'))
      .toBe('Application window');
  });
});

describe('isSameWindow', () => {
  test('identifies the selected row by handle and process', () => {
    expect(isSameWindow(window({ title: 'Old title' }), window({ title: 'New title' }))).toBe(true);
  });

  test('different rows are not the same window', () => {
    expect(isSameWindow(window({ handle: 1 }), window({ handle: 2 }))).toBe(false);
    expect(isSameWindow(window({ process_id: 1 }), window({ process_id: 2 }))).toBe(false);
  });

  test('whole system is only equal to whole system', () => {
    expect(isSameWindow(null, null)).toBe(true);
    expect(isSameWindow(window(), null)).toBe(false);
    expect(isSameWindow(null, window())).toBe(false);
  });
});
