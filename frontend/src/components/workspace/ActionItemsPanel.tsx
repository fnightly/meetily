'use client';

import { useMemo, useState } from 'react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';
import { AlertCircle, CalendarClock, Check, ListChecks, Plus, Sparkles, Trash2, UserRound } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';
import { useActionItems } from '@/hooks/useActionItems';
import { extractActionItems, isOverdue, type ActionItem } from '@/lib/action-items';

type Filter = 'open' | 'done' | 'all';

function ItemRow({
  item,
  showMeeting,
  onToggle,
  onSave,
  onDelete,
}: {
  item: ActionItem;
  showMeeting: boolean;
  onToggle: () => void;
  onSave: (patch: Partial<Pick<ActionItem, 'text' | 'owner' | 'due_date'>>) => void;
  onDelete: () => void;
}) {
  const { t } = useI18n();
  const router = useRouter();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(item.text);
  const [owner, setOwner] = useState(item.owner ?? '');
  const [due, setDue] = useState(item.due_date ?? '');
  const overdue = isOverdue(item);

  const commit = () => {
    onSave({ text: text.trim() || item.text, owner: owner.trim() || null, due_date: due.trim() || null });
    setEditing(false);
  };

  return (
    <li className="group flex items-start gap-3 rounded-lg px-2 py-2 hover:bg-slate-50">
      <button
        type="button"
        role="checkbox"
        aria-checked={item.done}
        aria-label={item.done ? t('tasks.markOpen') : t('tasks.markDone')}
        onClick={onToggle}
        className={`mt-0.5 flex h-[18px] w-[18px] flex-shrink-0 items-center justify-center rounded-[5px] border transition-colors ${item.done ? 'border-emerald-500 bg-emerald-500 text-white' : 'border-slate-300 bg-white hover:border-blue-400'}`}
      >
        {item.done && <Check className="h-3 w-3" strokeWidth={3} />}
      </button>
      <div className="min-w-0 flex-1">
        {editing ? (
          <div className="space-y-1.5">
            <input autoFocus value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') commit(); if (e.key === 'Escape') setEditing(false); }}
              className="h-8 w-full rounded-md border border-blue-300 px-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500/30" aria-label={t('tasks.text')} />
            <div className="flex gap-1.5">
              <input value={owner} onChange={(e) => setOwner(e.target.value)} placeholder={t('tasks.owner')} aria-label={t('tasks.owner')}
                className="h-7 w-32 rounded-md border border-slate-200 px-2 text-xs focus:outline-none focus:ring-2 focus:ring-blue-500/30" />
              <input value={due} onChange={(e) => setDue(e.target.value)} placeholder={t('tasks.duePlaceholder')} aria-label={t('tasks.due')}
                className="h-7 w-36 rounded-md border border-slate-200 px-2 text-xs focus:outline-none focus:ring-2 focus:ring-blue-500/30" />
              <Button size="sm" className="h-7" onClick={commit}>{t('common.save')}</Button>
              <Button size="sm" variant="ghost" className="h-7" onClick={() => setEditing(false)}>{t('common.cancel')}</Button>
            </div>
          </div>
        ) : (
          <button type="button" onClick={() => setEditing(true)} className="block w-full text-left">
            <span className={`block text-sm leading-snug ${item.done ? 'text-slate-400 line-through' : 'text-slate-800'}`}>{item.text}</span>
            <span className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px]">
              {item.owner && <span className="chip bg-blue-50 text-blue-700"><UserRound className="h-3 w-3" />{item.owner}</span>}
              {item.due_date && (
                <span className={`chip ${overdue ? 'bg-red-50 text-red-600' : 'bg-amber-50 text-amber-700'}`}>
                  <CalendarClock className="h-3 w-3" />{item.due_date}{overdue ? ` · ${t('tasks.overdue')}` : ''}
                </span>
              )}
              {item.source === 'summary' && <span className="chip bg-violet-50 text-violet-600"><Sparkles className="h-3 w-3" />{t('tasks.fromSummary')}</span>}
            </span>
          </button>
        )}
        {showMeeting && item.meeting_id && !editing && (
          <button
            type="button"
            onClick={() => router.push(`/meeting-details?id=${encodeURIComponent(item.meeting_id as string)}`)}
            className="mt-1 block max-w-full truncate text-[11px] text-slate-400 hover:text-blue-600"
          >
            {item.meeting_title ?? t('live.untitled')}
          </button>
        )}
      </div>
      {!editing && (
        <button type="button" onClick={onDelete} aria-label={t('common.delete')}
          className="rounded-md p-1 text-slate-300 opacity-0 transition-opacity hover:bg-red-50 hover:text-red-500 group-hover:opacity-100 focus-visible:opacity-100">
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      )}
    </li>
  );
}

/**
 * Follow-ups for one meeting (with "extract from summary") or for the whole
 * library (with open/done filters and meeting links).
 */
export function ActionItemsPanel({
  meetingId = null,
  summaryMarkdown,
  limit,
  compact = false,
}: {
  meetingId?: string | null;
  summaryMarkdown?: string | null;
  limit?: number;
  compact?: boolean;
}) {
  const { t } = useI18n();
  const { items, loading, error, reload, create, update, remove, importDrafts } = useActionItems(meetingId);
  const [filter, setFilter] = useState<Filter>('open');
  const [draft, setDraft] = useState('');
  const [extracting, setExtracting] = useState(false);

  // Mutation failures used to surface as unhandled rejections: the inline edits
  // and the tick/delete buttons are fire-and-forget.
  const reportSaveFailure = (e: unknown) => toast.error(t('tasks.saveFailed'), { description: String(e) });
  const toggleItem = (item: ActionItem) => { void update(item.id, { done: !item.done }).catch(reportSaveFailure); };
  const saveItem = (item: ActionItem, patch: Partial<Pick<ActionItem, 'text' | 'owner' | 'due_date'>>) => {
    void update(item.id, patch).catch(reportSaveFailure);
  };
  const removeItem = (item: ActionItem) => { void remove(item.id).catch(reportSaveFailure); };

  const visible = useMemo(() => {
    const list = items.filter((it) => (filter === 'all' ? true : filter === 'done' ? it.done : !it.done));
    return limit ? list.slice(0, limit) : list;
  }, [items, filter, limit]);
  const openCount = items.filter((it) => !it.done).length;
  const doneCount = items.length - openCount;

  const add = async () => {
    const parsed = draft.trim();
    if (!parsed) return;
    try {
      const { parseBullet } = await import('@/lib/action-items');
      const d = parseBullet(parsed) ?? { text: parsed, owner: null, due_date: null };
      await create(d);
      setDraft('');
    } catch (e) {
      toast.error(t('tasks.saveFailed'), { description: String(e) });
    }
  };

  const extract = async () => {
    if (!meetingId) return;
    const drafts = extractActionItems(summaryMarkdown);
    if (drafts.length === 0) {
      toast.info(t('tasks.noneInSummary'));
      return;
    }
    setExtracting(true);
    try {
      const outcome = await importDrafts(meetingId, drafts, true);
      toast.success(outcome.imported > 0 ? t('tasks.imported', { count: outcome.imported }) : t('tasks.nothingNew'));
    } catch (e) {
      toast.error(t('tasks.saveFailed'), { description: String(e) });
    } finally {
      setExtracting(false);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      {!compact && (
        <div className="flex flex-wrap items-center gap-2 pb-3">
          <div className="inline-flex rounded-lg bg-slate-100 p-0.5 text-xs">
            {(['open', 'done', 'all'] as Filter[]).map((f) => (
              <button key={f} type="button" onClick={() => setFilter(f)}
                className={`rounded-md px-2.5 py-1 font-medium transition-colors ${filter === f ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-500 hover:text-slate-800'}`}>
                {t(`tasks.filter.${f}`)}{f === 'open' ? ` ${openCount}` : f === 'done' ? ` ${doneCount}` : ''}
              </button>
            ))}
          </div>
          <div className="flex-1" />
          {meetingId && (
            <Button size="sm" variant="outline" disabled={extracting || !summaryMarkdown} onClick={() => void extract()}
              className="border-violet-200 text-violet-700 hover:bg-violet-50">
              <Sparkles />{t('tasks.extract')}
            </Button>
          )}
        </div>
      )}

      <div className="flex items-center gap-2 rounded-lg border border-slate-200 bg-white px-2.5 focus-within:border-blue-300 focus-within:ring-2 focus-within:ring-blue-500/20">
        <Plus className="h-4 w-4 text-slate-400" />
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void add(); }}
          placeholder={t('tasks.addPlaceholder')}
          aria-label={t('tasks.add')}
          className="h-9 flex-1 bg-transparent text-sm focus:outline-none"
        />
      </div>

      <div className="mt-2 min-h-0 flex-1 overflow-y-auto">
        {loading ? (
          <div role="status" className="space-y-1" aria-busy="true" aria-label={t('common.loading')}>
            {[0, 1, 2].map((i) => (
              <div key={i} className="flex items-start gap-3 px-2 py-2">
                <div className="mt-0.5 h-[18px] w-[18px] flex-shrink-0 animate-pulse rounded-[5px] bg-slate-200" />
                <div className="min-w-0 flex-1 space-y-1.5">
                  <div className="h-3 w-3/4 animate-pulse rounded bg-slate-200" />
                  <div className="h-2.5 w-1/3 animate-pulse rounded bg-slate-100" />
                </div>
              </div>
            ))}
          </div>
        ) : visible.length === 0 && error ? (
          <div role="alert" className="flex flex-col items-center py-10 text-center">
            <span className="mb-2 flex h-10 w-10 items-center justify-center rounded-full bg-red-50 text-red-500"><AlertCircle className="h-5 w-5" /></span>
            <p className="max-w-sm break-words text-xs text-red-600">{error}</p>
            <Button size="sm" variant="outline" className="mt-3" onClick={() => void reload()}>{t('common.retry')}</Button>
          </div>
        ) : visible.length === 0 ? (
          <div className="flex flex-col items-center py-10 text-center">
            <span className="mb-2 flex h-10 w-10 items-center justify-center rounded-full bg-emerald-50 text-emerald-500"><ListChecks className="h-5 w-5" /></span>
            <p className="text-sm font-medium text-slate-600">{filter === 'done' ? t('tasks.emptyDone') : t('tasks.emptyOpen')}</p>
            {meetingId && filter !== 'done' && summaryMarkdown && <p className="mt-1 text-xs text-slate-400">{t('tasks.emptyHintSummary')}</p>}
          </div>
        ) : (
          <>
            {error && (
              <div role="alert" className="mb-2 flex items-center gap-2 rounded-md bg-red-50 px-2 py-1 text-[11px] text-red-700">
                <AlertCircle className="h-3.5 w-3.5 flex-shrink-0" />
                <span className="min-w-0 flex-1 truncate" title={error}>{error}</span>
                <button type="button" onClick={() => void reload()} className="font-medium underline">{t('common.retry')}</button>
              </div>
            )}
            <ul className="space-y-0.5">
              {visible.map((item) => (
                <ItemRow
                  key={item.id}
                  item={item}
                  showMeeting={!meetingId}
                  onToggle={() => toggleItem(item)}
                  onSave={(patch) => saveItem(item, patch)}
                  onDelete={() => removeItem(item)}
                />
              ))}
            </ul>
          </>
        )}
      </div>
    </div>
  );
}
