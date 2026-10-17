/**
 * The sidebar's single list of meetings and recordings.
 *
 * A meeting and its audio are two records of one thing: a row in the database and
 * a folder on disk. The sidebar shows one row per thing, so the two lists are
 * merged here — in a pure function, so the rule that decides when they are the
 * same item can be tested without rendering a component.
 */

import type { RecordingEntry } from './recordings';

/** A meeting, as the merged list needs it. */
export interface SidebarMeeting {
  id: string;
  title: string;
  /** Folder the meeting's recording lives in, when the backend reports one. */
  folderPath?: string | null;
}

/** One row of the merged sidebar list. */
export interface SidebarEntry {
  /** React key; stays unique even when a meeting and a recording became one row. */
  key: string;
  /** The meeting behind the row, or null when the recording has no meeting row. */
  meetingId: string | null;
  folderPath: string | null;
  title: string;
  /**
   * Facts known only from the recording on disk. All null/false for a meeting
   * whose recording was moved or deleted.
   */
  createdAt: string | null;
  durationSeconds: number | null;
  audioBytes: number | null;
  hasTranscript: boolean;
  hasSummary: boolean;
  /** Seconds since the epoch when a recording dates the row, else null. */
  sortKey: number | null;
}

/**
 * Fold away the differences that do not make two paths different folders.
 *
 * Both sides come from the same Rust path handling, but the database keeps its own
 * copy, which can differ in separators or carry a trailing one. Case is
 * deliberately not folded: on a case-sensitive filesystem that would merge two
 * genuinely different folders.
 */
function folderKey(path: string | null | undefined): string | null {
  if (!path) return null;

  const normalized = path.trim().replace(/\\/g, '/').replace(/\/+$/, '');
  return normalized.length > 0 ? normalized : null;
}

/**
 * Merge meetings and recordings into one list, newest first.
 *
 * A meeting claims the recording in its folder, so the pair renders as one row.
 * Meetings with no recording are kept — their audio was moved or deleted, which is
 * exactly when the meeting is still worth opening — but nothing dates them, so
 * they follow the dated rows in their database order rather than being placed by a
 * guessed time.
 */
export function mergeSidebarEntries(
  meetings: SidebarMeeting[],
  recordings: RecordingEntry[],
): SidebarEntry[] {
  const recordingsByFolder = new Map<string, RecordingEntry[]>();
  for (const recording of recordings) {
    const key = folderKey(recording.folder_path);
    if (!key) continue;

    const existing = recordingsByFolder.get(key);
    if (existing) existing.push(recording);
    else recordingsByFolder.set(key, [recording]);
  }

  const claimed = new Set<RecordingEntry>();

  const meetingEntries: SidebarEntry[] = meetings.map((meeting) => {
    const key = folderKey(meeting.folderPath);
    // One recording backs one meeting. A second meeting pointing at the same
    // folder keeps its own row rather than taking the recording from the first.
    const recording = key
      ? recordingsByFolder.get(key)?.find((candidate) => !claimed.has(candidate))
      : undefined;
    if (recording) claimed.add(recording);

    return {
      key: `meeting:${meeting.id}`,
      meetingId: meeting.id,
      folderPath: meeting.folderPath ?? null,
      // The meeting's title wins on a merged row: it is the name the details page
      // shows, and the one an edit of the meeting updates.
      title: meeting.title,
      createdAt: recording?.created_at ?? null,
      durationSeconds: recording?.duration_seconds ?? null,
      audioBytes: recording?.audio_bytes ?? null,
      hasTranscript: recording?.has_transcript ?? false,
      hasSummary: recording?.has_summary ?? false,
      sortKey: recording?.sort_key ?? null,
    };
  });

  const orphanEntries: SidebarEntry[] = recordings
    .filter((recording) => !claimed.has(recording))
    .map((recording) => ({
      key: `recording:${recording.folder_path}`,
      meetingId: null,
      folderPath: recording.folder_path,
      title: recording.meeting_name,
      createdAt: recording.created_at,
      durationSeconds: recording.duration_seconds,
      audioBytes: recording.audio_bytes,
      hasTranscript: recording.has_transcript,
      hasSummary: recording.has_summary,
      sortKey: recording.sort_key ?? null,
    }));

  const all = [...meetingEntries, ...orphanEntries];
  const dated = all.filter((entry) => entry.sortKey !== null);
  const undated = all.filter((entry) => entry.sortKey === null);

  // `sort` is stable, so entries sharing a timestamp keep the order of the lists
  // they came from.
  dated.sort((a, b) => (b.sortKey as number) - (a.sortKey as number));

  return [...dated, ...undated];
}
