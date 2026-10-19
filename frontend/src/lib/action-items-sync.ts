import { invoke } from '@tauri-apps/api/core';
import type { SummaryProcessResponse } from '@/types';
import { extractActionItems } from '@/lib/action-items';
import { summaryToMarkdown } from '@/lib/meeting-export';
import { parseSummaryContent } from '@/lib/summary-content';

/** Import a meeting's summary follow-ups. Non-forced imports run once per meeting. */
export async function syncActionItemsFromMarkdown(meetingId: string, markdown: string | null | undefined, force = false) {
  const drafts = extractActionItems(markdown);
  if (drafts.length === 0 && !force) {
    // Still record the import so an empty summary is not re-parsed forever.
    return invoke<{ imported: number; skipped: boolean }>('api_import_action_items', { meetingId, items: [], force: false });
  }
  const outcome = await invoke<{ imported: number; skipped: boolean }>('api_import_action_items', { meetingId, items: drafts, force });
  if (outcome.imported > 0) window.dispatchEvent(new CustomEvent('meetily:action-items-changed'));
  return outcome;
}

export async function summaryMarkdownFor(meetingId: string): Promise<string | null> {
  try {
    const response = await invoke<SummaryProcessResponse>('api_get_summary', { meetingId });
    if (response.status === 'idle') return null;
    const summary = parseSummaryContent(response.data);
    const md = summaryToMarkdown(summary);
    return md.trim() ? md : null;
  } catch {
    return null;
  }
}

/**
 * Meetings inspected per session. The pass exists for summaries generated before
 * action items were mined automatically; a bounded sample covers the likely ones
 * without walking the whole library, and any meeting still gets synced when it is
 * opened (meeting details runs the same import for its own summary).
 */
const BACKFILL_MAX_MEETINGS = 10;

/** Wait before starting, so the page that triggered the pass has painted and loaded. */
const BACKFILL_DELAY_MS = 2000;

let backfillStarted = false;

/**
 * Run `run` on the next idle period. Chrome/Edge expose requestIdleCallback;
 * elsewhere the delay above has already elapsed and a timer is enough.
 */
function whenIdle(run: () => void): void {
  const idleWindow = window as Window & {
    requestIdleCallback?: (callback: () => void, options?: { timeout?: number }) => number;
  };
  if (typeof idleWindow.requestIdleCallback === 'function') {
    idleWindow.requestIdleCallback(run, { timeout: 1000 });
    return;
  }
  setTimeout(run, 0);
}

/**
 * One pass per app session over meetings that have a summary but were never
 * mined for action items.
 *
 * Deferred on purpose: callers invoke this from a page mount, where the initial
 * action-item list is still loading. Waiting for idle and then processing a
 * bounded number of meetings keeps opening Tasks/Ask responsive instead of
 * queueing dozens of `api_get_summary` calls ahead of the first paint.
 */
export async function backfillActionItems(maxMeetings = BACKFILL_MAX_MEETINGS): Promise<void> {
  if (typeof window === 'undefined' || backfillStarted) return;
  backfillStarted = true;

  await new Promise<void>((resolve) => setTimeout(resolve, BACKFILL_DELAY_MS));
  await new Promise<void>((resolve) => whenIdle(() => resolve()));

  try {
    const meetings = await invoke<Array<{ id: string }>>('api_get_meetings');
    const bounded = meetings.slice(0, Math.max(0, maxMeetings));
    for (const meeting of bounded) {
      const markdown = await summaryMarkdownFor(meeting.id);
      if (markdown) await syncActionItemsFromMarkdown(meeting.id, markdown, false).catch(() => {});
    }
  } catch {
    // Best effort; the user can still extract per meeting.
  }
}
