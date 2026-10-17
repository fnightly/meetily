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

let backfillStarted = false;

/**
 * One pass per app session over meetings that have a summary but were never
 * mined for action items (summaries generated before this feature existed).
 */
export async function backfillActionItems(maxMeetings = 40): Promise<void> {
  if (backfillStarted) return;
  backfillStarted = true;
  try {
    const meetings = await invoke<Array<{ id: string }>>('api_get_meetings');
    for (const m of meetings.slice(0, maxMeetings)) {
      const md = await summaryMarkdownFor(m.id);
      if (md) await syncActionItemsFromMarkdown(m.id, md, false).catch(() => {});
    }
  } catch {
    // Best effort; the user can still extract per meeting.
  }
}
