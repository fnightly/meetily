import { describe, expect, test } from 'bun:test';
import { buildHomeOverview, greetingKey, splitDuration, startOfWeek } from '@/lib/home-overview';
import type { SidebarEntry } from '@/lib/sidebar-entries';

const entry = (over: Partial<SidebarEntry>): SidebarEntry => ({
  key: Math.random().toString(36),
  meetingId: 'm',
  folderPath: null,
  title: 't',
  createdAt: null,
  durationSeconds: null,
  audioBytes: null,
  hasTranscript: false,
  hasSummary: false,
  sortKey: null,
  ...over,
});

describe('home overview', () => {
  test('greeting buckets', () => {
    expect(greetingKey(new Date(2026, 0, 1, 3))).toBe('night');
    expect(greetingKey(new Date(2026, 0, 1, 9))).toBe('morning');
    expect(greetingKey(new Date(2026, 0, 1, 12))).toBe('noon');
    expect(greetingKey(new Date(2026, 0, 1, 15))).toBe('afternoon');
    expect(greetingKey(new Date(2026, 0, 1, 20))).toBe('evening');
  });

  test('week starts on Monday', () => {
    // 2026-10-18 is a Sunday → week starts Monday 2026-10-12
    const s = startOfWeek(new Date(2026, 9, 18, 15));
    expect([s.getFullYear(), s.getMonth(), s.getDate(), s.getHours()]).toEqual([2026, 9, 12, 0]);
  });

  test('counts week activity and follow-ups', () => {
    const now = new Date(2026, 9, 15, 12); // Thursday
    const ts = (d: Date) => d.getTime() / 1000;
    const o = buildHomeOverview(
      [
        entry({ sortKey: ts(new Date(2026, 9, 14)), durationSeconds: 1800, hasTranscript: true, hasSummary: true }),
        entry({ sortKey: ts(new Date(2026, 9, 13)), durationSeconds: 600, hasTranscript: true }),
        entry({ sortKey: ts(new Date(2026, 9, 1)), durationSeconds: 900, audioBytes: 10, meetingId: null }),
        entry({ sortKey: null, hasTranscript: true, meetingId: null }),
      ],
      now,
      2,
    );
    expect(o.weekCount).toBe(2);
    expect(o.weekSeconds).toBe(2400);
    expect(o.needSummary).toBe(1);
    expect(o.needTranscript).toBe(1);
    expect(o.total).toBe(4);
    expect(o.recent.map((e) => e.durationSeconds)).toEqual([1800, 600]);
  });

  test('splitDuration rounds to minutes', () => {
    expect(splitDuration(3930)).toEqual({ h: 1, m: 6 });
    expect(splitDuration(-5)).toEqual({ h: 0, m: 0 });
  });
});
