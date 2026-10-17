/**
 * Filtering, sorting, date grouping and statistics for the recordings library.
 *
 * Pure functions over `RecordingEntry` so the rules (what "untranscribed"
 * means, which bucket a recording falls in) are tested without rendering.
 */

import type { RecordingEntry } from './recordings';

export type LibraryFilter = 'all' | 'transcribed' | 'untranscribed' | 'summarized' | 'unsummarized' | 'noAudio';
export type LibrarySort = 'newest' | 'oldest' | 'longest' | 'largest' | 'name';
export type DateBucket = 'today' | 'yesterday' | 'thisWeek' | 'thisMonth' | 'earlier' | 'undated';

export const LIBRARY_FILTERS: LibraryFilter[] = [
  'all',
  'transcribed',
  'untranscribed',
  'summarized',
  'unsummarized',
  'noAudio',
];
export const LIBRARY_SORTS: LibrarySort[] = ['newest', 'oldest', 'longest', 'largest', 'name'];

export function matchesFilter(entry: RecordingEntry, filter: LibraryFilter): boolean {
  switch (filter) {
    case 'all':
      return true;
    case 'transcribed':
      return entry.has_transcript;
    case 'untranscribed':
      return !entry.has_transcript && !!entry.audio_file;
    case 'summarized':
      return entry.has_summary;
    case 'unsummarized':
      return entry.has_transcript && !entry.has_summary;
    case 'noAudio':
      return !entry.audio_file;
  }
}

/**
 * Case-insensitive match on the title and folder name. `transcriptHits` holds
 * folder paths whose transcript matched a full-text search, so a recording can
 * be found by something that was said in it.
 */
export function matchesQuery(
  entry: RecordingEntry,
  query: string,
  transcriptHits?: ReadonlySet<string>,
): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  if (entry.meeting_name.toLowerCase().includes(q)) return true;
  if (entry.folder_name.toLowerCase().includes(q)) return true;
  return transcriptHits?.has(entry.folder_path) ?? false;
}

export function sortRecordings(list: RecordingEntry[], sort: LibrarySort): RecordingEntry[] {
  const out = [...list];
  const byNewest = (a: RecordingEntry, b: RecordingEntry) => (b.sort_key ?? 0) - (a.sort_key ?? 0);
  switch (sort) {
    case 'newest':
      return out.sort(byNewest);
    case 'oldest':
      return out.sort((a, b) => (a.sort_key ?? 0) - (b.sort_key ?? 0));
    case 'longest':
      return out.sort((a, b) => (b.duration_seconds ?? -1) - (a.duration_seconds ?? -1) || byNewest(a, b));
    case 'largest':
      return out.sort((a, b) => (b.audio_bytes ?? -1) - (a.audio_bytes ?? -1) || byNewest(a, b));
    case 'name':
      return out.sort(
        (a, b) => a.meeting_name.localeCompare(b.meeting_name, ['zh-CN', 'en'], { numeric: true }) || byNewest(a, b),
      );
  }
}

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Which date heading a recording belongs under, in local time. */
export function dateBucket(sortKeySeconds: number | null | undefined, now: Date = new Date()): DateBucket {
  if (!sortKeySeconds || !Number.isFinite(sortKeySeconds) || sortKeySeconds <= 0) return 'undated';
  const t = sortKeySeconds * 1000;
  const today = startOfDay(now);
  const day = 24 * 3600 * 1000;
  if (t >= today) return 'today';
  if (t >= today - day) return 'yesterday';
  // Week starts on Monday.
  const weekday = (now.getDay() + 6) % 7;
  if (t >= today - weekday * day) return 'thisWeek';
  if (t >= new Date(now.getFullYear(), now.getMonth(), 1).getTime()) return 'thisMonth';
  return 'earlier';
}

export interface DateGroup {
  bucket: DateBucket;
  items: RecordingEntry[];
}

/** Group an already-sorted list, keeping the list's order of first appearance. */
export function groupByDate(list: RecordingEntry[], now: Date = new Date()): DateGroup[] {
  const groups: DateGroup[] = [];
  const index = new Map<DateBucket, DateGroup>();
  for (const entry of list) {
    const bucket = dateBucket(entry.sort_key, now);
    let group = index.get(bucket);
    if (!group) {
      group = { bucket, items: [] };
      index.set(bucket, group);
      groups.push(group);
    }
    group.items.push(entry);
  }
  return groups;
}

export interface LibraryStats {
  count: number;
  totalSeconds: number;
  totalBytes: number;
  transcribed: number;
  summarized: number;
}

export function libraryStats(list: RecordingEntry[]): LibraryStats {
  return list.reduce<LibraryStats>(
    (acc, e) => ({
      count: acc.count + 1,
      totalSeconds: acc.totalSeconds + (e.duration_seconds ?? 0),
      totalBytes: acc.totalBytes + (e.audio_bytes ?? 0),
      transcribed: acc.transcribed + (e.has_transcript ? 1 : 0),
      summarized: acc.summarized + (e.has_summary ? 1 : 0),
    }),
    { count: 0, totalSeconds: 0, totalBytes: 0, transcribed: 0, summarized: 0 },
  );
}

/** Long totals: `3 小时 20 分` style is done by the caller; this gives h/m parts. */
export function splitHoursMinutes(totalSeconds: number): { hours: number; minutes: number } {
  const minutes = Math.round(Math.max(0, totalSeconds) / 60);
  return { hours: Math.floor(minutes / 60), minutes: minutes % 60 };
}

export interface FolderLookup {
  get(folder: string): string | undefined;
  has(folder: string): boolean;
}

/** Folder path → meeting id, from the merged sidebar entries. */
export function meetingIdsByFolder(
  entries: Array<{ folderPath: string | null; meetingId: string | null }>,
): FolderLookup {
  const norm = (p: string) => p.trim().replace(/\\/g, '/').replace(/\/+$/, '');
  const map = new Map<string, string>();
  for (const e of entries) {
    if (e.folderPath && e.meetingId) map.set(norm(e.folderPath), e.meetingId);
  }
  return {
    get: (folder: string) => map.get(norm(folder)),
    has: (folder: string) => map.has(norm(folder)),
  };
}
