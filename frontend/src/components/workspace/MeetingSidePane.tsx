'use client';

import { useState, type ReactNode } from 'react';
import { ListChecks, MessageCircleQuestion, Sparkles } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { useActionItems } from '@/hooks/useActionItems';
import { ActionItemsPanel } from './ActionItemsPanel';
import { AskPanel } from './AskPanel';

export type SidePaneTab = 'summary' | 'tasks' | 'ask';

/**
 * Right-hand pane of a meeting: the summary editor plus the meeting's
 * follow-ups and a Q&A box. The summary stays mounted while hidden so the
 * editor keeps its unsaved state and generation progress.
 */
export function MeetingSidePane({
  meetingId,
  summary,
  summaryMarkdown,
}: {
  meetingId: string;
  summary: ReactNode;
  summaryMarkdown: string | null;
}) {
  const { t } = useI18n();
  const [tab, setTab] = useState<SidePaneTab>('summary');
  const { items } = useActionItems(meetingId);
  const openCount = items.filter((it) => !it.done).length;

  const tabs: Array<{ id: SidePaneTab; label: string; icon: typeof Sparkles; badge?: number }> = [
    { id: 'summary', label: t('meeting.pane.summary'), icon: Sparkles },
    { id: 'tasks', label: t('meeting.pane.tasks'), icon: ListChecks, badge: openCount },
    { id: 'ask', label: t('meeting.pane.ask'), icon: MessageCircleQuestion },
  ];

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      <div role="tablist" aria-label={t('meeting.summaryPane')} className="flex shrink-0 items-center gap-1 border-b border-slate-200 bg-white px-3 pt-2">
        {tabs.map(({ id, label, icon: Icon, badge }) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            onClick={() => setTab(id)}
            className={`relative -mb-px inline-flex items-center gap-1.5 border-b-2 px-3 pb-2 pt-1 text-sm font-medium transition-colors ${
              tab === id ? 'border-blue-600 text-blue-700' : 'border-transparent text-slate-500 hover:text-slate-800'
            }`}
          >
            <Icon className="h-4 w-4" />
            {label}
            {!!badge && badge > 0 && (
              <span className="rounded-full bg-blue-100 px-1.5 text-[11px] font-semibold tabular-nums text-blue-700">{badge}</span>
            )}
          </button>
        ))}
      </div>
      <div className={tab === 'summary' ? 'flex min-h-0 flex-1 flex-col' : 'hidden'}>{summary}</div>
      {tab === 'tasks' && (
        <div className="min-h-0 flex-1 overflow-hidden bg-slate-50/40 p-4">
          <ActionItemsPanel meetingId={meetingId} summaryMarkdown={summaryMarkdown} />
        </div>
      )}
      {tab === 'ask' && (
        <div className="min-h-0 flex-1 overflow-hidden">
          <AskPanel meetingId={meetingId} className="h-full" />
        </div>
      )}
    </div>
  );
}
