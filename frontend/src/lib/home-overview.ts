/**
 * Pure helpers behind the home dashboard: greeting, "this week" numbers and the
 * follow-up queue. Kept out of the component so they can be tested without a DOM.
 */
import type { SidebarEntry } from '@/lib/sidebar-entries';

export type GreetingKey = 'night' | 'morning' | 'noon' | 'afternoon' | 'evening';

export function greetingKey(date: Date): GreetingKey {
  const h = date.getHours();
  if (h < 5) return 'night';
  if (h < 11) return 'morning';
  if (h < 13) return 'noon';
  if (h < 18) return 'afternoon';
  return 'evening';
}

/** Monday 00:00 of the week containing `now`, local time. */
export function startOfWeek(now: Date): Date {
  const d = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const offset = (d.getDay() + 6) % 7;
  d.setDate(d.getDate() - offset);
  return d;
}

export interface HomeOverview {
  weekCount: number;
  weekSeconds: number;
  total: number;
  /** Recorded but never transcribed. */
  needTranscript: number;
  /** Transcribed but no summary yet — the most common follow-up. */
  needSummary: number;
  /** Most recent entries first. */
  recent: SidebarEntry[];
}

export function buildHomeOverview(entries: SidebarEntry[], now: Date, recentLimit = 6): HomeOverview {
  const weekStart = startOfWeek(now).getTime() / 1000;
  let weekCount = 0;
  let weekSeconds = 0;
  let needTranscript = 0;
  let needSummary = 0;
  for (const e of entries) {
    if (e.sortKey !== null && e.sortKey >= weekStart) {
      weekCount += 1;
      weekSeconds += e.durationSeconds ?? 0;
    }
    if (!e.hasTranscript && e.audioBytes !== null) needTranscript += 1;
    else if (e.hasTranscript && !e.hasSummary && e.meetingId) needSummary += 1;
  }
  const recent = [...entries]
    .sort((a, b) => (b.sortKey ?? -Infinity) - (a.sortKey ?? -Infinity))
    .slice(0, recentLimit);
  return { weekCount, weekSeconds, total: entries.length, needTranscript, needSummary, recent };
}

/** "1 小时 5 分" style pieces; the caller localizes. */
export function splitDuration(seconds: number): { h: number; m: number } {
  const total = Math.max(0, Math.round(seconds / 60));
  return { h: Math.floor(total / 60), m: total % 60 };
}
