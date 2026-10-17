import { describe, expect, test } from 'bun:test';
import {
  dateBucket,
  groupByDate,
  libraryStats,
  matchesFilter,
  matchesQuery,
  meetingIdsByFolder,
  sortRecordings,
  splitHoursMinutes,
} from '../../src/lib/library-view';
import type { RecordingEntry } from '../../src/lib/recordings';

const now = new Date(2026, 2, 12, 15, 0, 0); // Thu 12 Mar 2026, local
const secs = (d: Date) => Math.floor(d.getTime() / 1000);

function rec(partial: Partial<RecordingEntry>): RecordingEntry {
  return {
    folder_name: partial.meeting_name ?? 'f',
    folder_path: `/r/${partial.meeting_name ?? 'f'}`,
    meeting_name: 'f',
    created_at: null,
    duration_seconds: null,
    audio_file: 'audio.m4a',
    audio_bytes: null,
    has_transcript: false,
    has_summary: false,
    status: null,
    sort_key: 0,
    ...partial,
  };
}

const a = rec({ meeting_name: '周会 2', sort_key: secs(new Date(2026, 2, 12, 9)), duration_seconds: 600, audio_bytes: 10, has_transcript: true });
const b = rec({ meeting_name: '周会 10', sort_key: secs(new Date(2026, 2, 11, 9)), duration_seconds: 3600, audio_bytes: 5, has_transcript: true, has_summary: true });
const c = rec({ meeting_name: 'Alpha', sort_key: secs(new Date(2026, 2, 9, 9)), audio_file: null });
const d = rec({ meeting_name: '客户访谈', sort_key: secs(new Date(2026, 1, 20, 9)), duration_seconds: 1200, audio_bytes: 50 });
const all = [c, a, d, b];

describe('filters', () => {
  test('each filter selects what its name says', () => {
    const pick = (f: Parameters<typeof matchesFilter>[1]) => all.filter((e) => matchesFilter(e, f)).map((e) => e.meeting_name);
    expect(pick('all')).toHaveLength(4);
    expect(pick('transcribed')).toEqual(['周会 2', '周会 10']);
    expect(pick('untranscribed')).toEqual(['客户访谈']); // Alpha has no audio: nothing to transcribe
    expect(pick('summarized')).toEqual(['周会 10']);
    expect(pick('unsummarized')).toEqual(['周会 2']);
    expect(pick('noAudio')).toEqual(['Alpha']);
  });

  test('query matches title, folder or a transcript hit', () => {
    expect(matchesQuery(a, '周会')).toBe(true);
    expect(matchesQuery(c, 'alp')).toBe(true);
    expect(matchesQuery(d, '预算')).toBe(false);
    expect(matchesQuery(d, '预算', new Set([d.folder_path]))).toBe(true);
    expect(matchesQuery(d, '  ')).toBe(true);
  });
});

describe('sorting', () => {
  test('by date, length, size and natural name order', () => {
    expect(sortRecordings(all, 'newest').map((e) => e.meeting_name)).toEqual(['周会 2', '周会 10', 'Alpha', '客户访谈']);
    expect(sortRecordings(all, 'oldest')[0].meeting_name).toBe('客户访谈');
    expect(sortRecordings(all, 'longest')[0].meeting_name).toBe('周会 10');
    expect(sortRecordings(all, 'largest')[0].meeting_name).toBe('客户访谈');
    const names = sortRecordings(all, 'name').map((e) => e.meeting_name);
    expect(names.indexOf('周会 2')).toBeLessThan(names.indexOf('周会 10'));
  });

  test('does not mutate the input', () => {
    const copy = [...all];
    sortRecordings(all, 'name');
    expect(all).toEqual(copy);
  });
});

describe('date buckets', () => {
  test('today / yesterday / this week / this month / earlier / undated', () => {
    expect(dateBucket(a.sort_key, now)).toBe('today');
    expect(dateBucket(b.sort_key, now)).toBe('yesterday');
    expect(dateBucket(c.sort_key, now)).toBe('thisWeek'); // Monday
    expect(dateBucket(secs(new Date(2026, 2, 2, 9)), now)).toBe('thisMonth');
    expect(dateBucket(d.sort_key, now)).toBe('earlier');
    expect(dateBucket(0, now)).toBe('undated');
  });

  test('groups follow the sorted order', () => {
    const groups = groupByDate(sortRecordings(all, 'newest'), now);
    expect(groups.map((g) => g.bucket)).toEqual(['today', 'yesterday', 'thisWeek', 'earlier']);
  });
});

describe('stats and lookup', () => {
  test('totals', () => {
    expect(libraryStats(all)).toEqual({ count: 4, totalSeconds: 5400, totalBytes: 65, transcribed: 2, summarized: 1 });
    expect(splitHoursMinutes(5400)).toEqual({ hours: 1, minutes: 30 });
  });

  test('folder lookup ignores separator differences', () => {
    const map = meetingIdsByFolder([
      { folderPath: 'C:\\rec\\周会\\', meetingId: 'm1' },
      { folderPath: null, meetingId: 'm2' },
    ]);
    expect(map.get('C:/rec/周会')).toBe('m1');
    expect(map.has('C:/rec/other')).toBe(false);
  });
});
