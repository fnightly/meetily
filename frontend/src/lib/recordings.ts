import { readStoredLocale } from '@/lib/i18n';
/**
 * Types and formatting for the recordings library.
 *
 * Kept out of the component so the formatting rules can be tested without
 * rendering, and so the Rust side's shape has one TypeScript counterpart.
 */

/** A recording as reported by `list_saved_recordings`. */
export interface RecordingEntry {
  folder_name: string;
  folder_path: string;
  meeting_name: string;
  created_at: string | null;
  duration_seconds: number | null;
  audio_file: string | null;
  /**
   * Absolute path to the audio file, used by inline playback (`read_audio_file`).
   * Optional because payloads from an older backend (and test fixtures) omit it;
   * null/undefined means playback is unavailable even when `audio_file` is set.
   */
  audio_path?: string | null;
  audio_bytes: number | null;
  has_transcript: boolean;
  has_summary: boolean;
  status: string | null;
  sort_key: number;
}

export interface RecordingsListView {
  folder: string;
  recordings: RecordingEntry[];
  total_bytes: number;
  folder_missing: boolean;
}

export interface RecordingMutationResult {
  folder_path: string;
  title: string;
  detail: string;
}

/**
 * Human-readable size.
 *
 * An unknown size renders as a dash rather than "0 B": a recording with no audio
 * must not look like an empty file.
 */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return '—';

  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;

  if (bytes >= GB) return `${(bytes / GB).toFixed(1)} GB`;
  if (bytes >= MB) return `${(bytes / MB).toFixed(1)} MB`;
  if (bytes >= KB) return `${(bytes / KB).toFixed(0)} KB`;
  return `${bytes} B`;
}

/** Format a duration in seconds as `m:ss` or `h:mm:ss`. */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return '—';
  if (!Number.isFinite(seconds) || seconds < 0) return '—';

  const total = Math.round(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;

  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
    : `${minutes}:${String(secs).padStart(2, '0')}`;
}

/**
 * Format the recording date.
 *
 * An unparseable value is shown as-is rather than as "Invalid Date", and a
 * missing one is stated outright.
 */
export function formatRecordedAt(createdAt: string | null | undefined): string {
  if (!createdAt) return 'Unknown date';

  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return createdAt;

  // Follow the interface language rather than the OS, so a Chinese UI on an English
  // system does not mix "Oct 16" into Chinese labels.
  return date.toLocaleString(readStoredLocale(), {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/** Whether there is audio to work with (name or absolute path present). */
function hasAudio(entry: RecordingEntry): boolean {
  return entry.audio_file !== null || entry.audio_path != null;
}

/** Whether a recording can be transcribed for the first time (audio, no transcript). */
export function canTranscribe(entry: RecordingEntry): boolean {
  return hasAudio(entry) && !entry.has_transcript;
}

/**
 * Whether a recording can be transcribed again.
 *
 * Re-transcribing replaces the existing transcript, so the caller must ask for
 * confirmation first; having audio is the only technical requirement.
 */
export function canRetranscribe(entry: RecordingEntry): boolean {
  return hasAudio(entry);
}

/** Whether a recording can be summarised (it has a transcript). */
export function canSummarise(entry: RecordingEntry): boolean {
  return entry.has_transcript;
}
