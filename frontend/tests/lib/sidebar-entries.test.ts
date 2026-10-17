import { describe, test, expect } from 'bun:test';
import {
  mergeSidebarEntries,
  type SidebarMeeting,
} from '../../src/lib/sidebar-entries';
import type { RecordingEntry } from '../../src/lib/recordings';

function meeting(overrides: Partial<SidebarMeeting> = {}): SidebarMeeting {
  return { id: 'meeting-a', title: 'Team Standup', folderPath: '/recordings/standup', ...overrides };
}

/** A minimal recording, overridable per test. */
function recording(overrides: Partial<RecordingEntry> = {}): RecordingEntry {
  return {
    folder_name: 'Team Standup_2026-05-01_10-00',
    folder_path: '/recordings/standup',
    meeting_name: 'Team Standup',
    created_at: '2026-05-01T10:00:00Z',
    duration_seconds: 1800,
    audio_file: 'audio.m4a',
    audio_bytes: 1024,
    has_transcript: false,
    has_summary: false,
    status: 'completed',
    sort_key: 1_000,
    ...overrides,
  };
}

describe('mergeSidebarEntries', () => {
  test('collapses a meeting and the recording in its folder into one row', () => {
    // The point of the merge: one meeting with audio is one item, not two.
    const entries = mergeSidebarEntries([meeting()], [recording()]);

    expect(entries).toHaveLength(1);
    expect(entries[0].meetingId).toBe('meeting-a');
    expect(entries[0].folderPath).toBe('/recordings/standup');
  });

  test('renders the meeting title with the recording facts', () => {
    // The database title is what the details page shows; the date, duration and
    // size exist only on the recording.
    const entries = mergeSidebarEntries(
      [meeting({ title: 'Renamed in the app' })],
      [recording({ meeting_name: 'Stale folder name', duration_seconds: 42, has_transcript: true })],
    );

    expect(entries[0].title).toBe('Renamed in the app');
    expect(entries[0].durationSeconds).toBe(42);
    expect(entries[0].hasTranscript).toBe(true);
  });

  test('keeps a recording that has no meeting as its own row', () => {
    const entries = mergeSidebarEntries([], [recording({ meeting_name: 'Imported audio' })]);

    expect(entries).toHaveLength(1);
    expect(entries[0].meetingId).toBeNull();
    expect(entries[0].title).toBe('Imported audio');
    expect(entries[0].key).toBe('recording:/recordings/standup');
  });

  test('keeps a meeting whose recording is gone as its own row', () => {
    // Moved or deleted audio is exactly when the meeting is still worth opening,
    // so the row survives with no recording facts attached.
    const entries = mergeSidebarEntries([meeting()], []);

    expect(entries).toHaveLength(1);
    expect(entries[0].meetingId).toBe('meeting-a');
    expect(entries[0].durationSeconds).toBeNull();
    expect(entries[0].audioBytes).toBeNull();
    expect(entries[0].sortKey).toBeNull();
  });

  test('never shows an unmatched meeting and recording as one row', () => {
    // Two folders, so two items: collapsing on anything but the path would hide
    // one of them.
    const entries = mergeSidebarEntries(
      [meeting({ folderPath: '/recordings/other' })],
      [recording()],
    );

    expect(entries).toHaveLength(2);
    expect(entries.map((entry) => entry.key).sort()).toEqual([
      'meeting:meeting-a',
      'recording:/recordings/standup',
    ]);
  });

  test('treats separator and trailing-slash differences as the same folder', () => {
    // The database copy of the path is not always written the way the folder scan
    // writes it.
    const entries = mergeSidebarEntries(
      [meeting({ folderPath: 'D:\\meetily\\standup\\' })],
      [recording({ folder_path: 'D:/meetily/standup' })],
    );

    expect(entries).toHaveLength(1);
    expect(entries[0].meetingId).toBe('meeting-a');
  });

  test('does not merge folders that differ only in case', () => {
    // A case-folded match would merge two real folders on a case-sensitive
    // filesystem, so the comparison stays exact.
    const entries = mergeSidebarEntries(
      [meeting({ folderPath: '/recordings/Standup' })],
      [recording({ folder_path: '/recordings/standup' })],
    );

    expect(entries).toHaveLength(2);
  });

  test('lets a meeting with no folder claim nothing', () => {
    const entries = mergeSidebarEntries([meeting({ folderPath: null })], [recording()]);

    expect(entries).toHaveLength(2);
    expect(entries.filter((entry) => entry.meetingId === null)).toHaveLength(1);
  });

  test('gives one recording to one meeting at most', () => {
    // Two meetings cannot both own the same folder; the second keeps its row
    // rather than duplicating the recording.
    const entries = mergeSidebarEntries(
      [meeting({ id: 'a' }), meeting({ id: 'b' })],
      [recording()],
    );

    expect(entries).toHaveLength(2);
    const merged = entries.filter((entry) => entry.durationSeconds !== null);
    expect(merged).toHaveLength(1);
  });

  test('orders dated rows newest first and leaves undated ones at the end', () => {
    const entries = mergeSidebarEntries(
      [meeting({ id: 'no-audio', folderPath: null }), meeting({ id: 'older', folderPath: '/r/older' })],
      [
        recording({ folder_path: '/r/newest', meeting_name: 'Newest', sort_key: 300 }),
        recording({ folder_path: '/r/older', sort_key: 200 }),
        recording({ folder_path: '/r/oldest', meeting_name: 'Oldest', sort_key: 100 }),
      ],
    );

    expect(entries.map((entry) => entry.title)).toEqual([
      'Newest',
      'Team Standup',
      'Oldest',
      'Team Standup',
    ]);
    expect(entries[3].meetingId).toBe('no-audio');
  });
});
