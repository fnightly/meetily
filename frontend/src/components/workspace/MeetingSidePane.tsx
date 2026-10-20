'use client';

import { useEffect, useState, type ReactNode } from 'react';
import dynamic from 'next/dynamic';
import { ListChecks, Loader2, MessageCircleQuestion, Sparkles, StickyNote } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';

export type SidePaneTab = 'summary' | 'notes' | 'tasks' | 'ask';

function PaneLoading() {
  const { t } = useI18n();
  return (
    <div
      role="status"
      className="flex h-full items-center justify-center gap-2 p-6 text-sm text-slate-500"
    >
      <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
      {t('common.loading')}
    </div>
  );
}

// Only needed when their tab is opened: importing them lazily keeps the code
// (and, for tasks, the action-item query) out of every meeting page load.
const ActionItemsPanel = dynamic(
  () => import('./ActionItemsPanel').then((mod) => mod.ActionItemsPanel),
  { ssr: false, loading: () => <PaneLoading /> },
);
const AskPanel = dynamic(
  () => import('./AskPanel').then((mod) => mod.AskPanel),
  { ssr: false, loading: () => <PaneLoading /> },
);
// The notes editor pulls in the Markdown renderer; load it only when the 笔记 tab
// is opened so a meeting page that never touches notes does not pay for it.
const loadNotesEditor = () => import('@/app/notes/MeetingNotesEditor');
const MeetingNotesEditor = dynamic(
  () => loadNotesEditor().then((mod) => mod.MeetingNotesEditor),
  { ssr: false, loading: () => <PaneLoading /> },
);

/**
 * Right-hand pane of a meeting: the summary editor, the user's own notes, the
 * meeting's follow-ups, and a Q&A box.
 *
 * The summary stays mounted while hidden so the editor keeps its unsaved state
 * and generation progress. Notes are mounted on first open and then kept
 * mounted for the same reason; before that opening the tab, no notes are read.
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
  // Tabs are scoped to a meeting: carrying them across a meeting switch would
  // leave the pane on 笔记 with no editor mounted for the new meeting.
  const [tabState, setTabState] = useState<{ meetingId: string; tab: SidePaneTab }>({
    meetingId,
    tab: 'summary',
  });
  const tab: SidePaneTab = tabState.meetingId === meetingId ? tabState.tab : 'summary';
  const [notesMeetingId, setNotesMeetingId] = useState<string | null>(null);
  const [notesDirty, setNotesDirty] = useState(false);
  const notesMounted = notesMeetingId === meetingId;

  // A different meeting needs a fresh editor: otherwise one meeting's blocks
  // could be saved into another's note.
  useEffect(() => {
    setNotesMeetingId(null);
    setNotesDirty(false);
  }, [meetingId]);

  const selectTab = (next: SidePaneTab) => {
    if (next === 'notes') setNotesMeetingId(meetingId);
    setTabState({ meetingId, tab: next });
  };

  // Start both the editor chunk and its note query while the pointer is over the
  // tab, so the click mostly renders already-available data.
  const warmNotes = () => {
    void loadNotesEditor().then((mod) => mod.primeMeetingNote(meetingId)).catch(() => {});
  };

  const tabs: Array<{ id: SidePaneTab; label: string; icon: typeof Sparkles }> = [
    { id: 'summary', label: t('meeting.pane.summary'), icon: Sparkles },
    { id: 'notes', label: t('meeting.pane.notes'), icon: StickyNote },
    { id: 'tasks', label: t('meeting.pane.tasks'), icon: ListChecks },
    { id: 'ask', label: t('meeting.pane.ask'), icon: MessageCircleQuestion },
  ];

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      <div role="tablist" aria-label={t('meeting.summaryPane')} className="flex shrink-0 items-center gap-1 border-b border-slate-200 bg-white px-3 pt-2">
        {tabs.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            onClick={() => selectTab(id)}
            onPointerEnter={id === 'notes' ? warmNotes : undefined}
            onFocus={id === 'notes' ? warmNotes : undefined}
            className={`relative -mb-px inline-flex items-center gap-1.5 border-b-2 px-3 pb-2 pt-1 text-sm font-medium transition-colors ${
              tab === id ? 'border-blue-600 text-blue-700' : 'border-transparent text-slate-500 hover:text-slate-800'
            }`}
          >
            <Icon className="h-4 w-4" />
            {label}
            {id === 'notes' && notesMounted && notesDirty && (
              <span
                role="img"
                aria-label={t('notes.unsaved')}
                title={t('notes.unsaved')}
                className="h-1.5 w-1.5 rounded-full bg-amber-500"
              />
            )}
          </button>
        ))}
      </div>
      <div className={tab === 'summary' ? 'flex min-h-0 flex-1 flex-col' : 'hidden'}>{summary}</div>
      {notesMounted && (
        <div className={tab === 'notes' ? 'flex min-h-0 flex-1 flex-col overflow-hidden bg-white' : 'hidden'}>
          <MeetingNotesEditor
            key={meetingId}
            meetingId={meetingId}
            embedded
            onDirtyChange={setNotesDirty}
          />
        </div>
      )}
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
