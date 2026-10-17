'use client';

import { useEffect, useRef, useState } from 'react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { CalendarDays, Check, ChevronLeft, Clock, Download, FileText, Pencil, Sparkles, StickyNote, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';
import { formatDuration } from '@/lib/recordings';
import { MeetingLabels } from '@/components/workspace/MeetingLabels';

interface MeetingHeaderProps {
  meetingId: string;
  title: string;
  createdAt?: string | null;
  segmentCount: number;
  durationSeconds: number | null;
  hasSummary: boolean;
  onTitleSaved: (title: string) => void;
  onExport: () => void;
}

/**
 * The meeting's identity bar: name (renamable in place), when it happened, how long,
 * and the actions that apply to the whole meeting rather than one pane.
 */
export function MeetingHeader({
  meetingId, title, createdAt, segmentCount, durationSeconds, hasSummary, onTitleSaved, onExport,
}: MeetingHeaderProps) {
  const { t, locale } = useI18n();
  const router = useRouter();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const [saving, setSaving] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { if (!editing) setDraft(title); }, [title, editing]);
  useEffect(() => { if (editing) inputRef.current?.select(); }, [editing]);

  const save = async () => {
    const next = draft.trim();
    if (!next || next === title) { setEditing(false); return; }
    setSaving(true);
    try {
      await invoke('api_save_meeting_title', { meetingId, title: next });
      onTitleSaved(next);
      toast.success(t('toast.titleUpdated'));
      setEditing(false);
    } catch (error) {
      toast.error(t('toast.titleUpdateFailed'), { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const date = createdAt ? new Date(createdAt) : null;
  const dateLabel = date && !Number.isNaN(date.getTime())
    ? date.toLocaleString(locale, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
    : null;

  return (
    <header className="shrink-0 border-b border-slate-200 bg-white px-6 py-3.5">
      <div className="flex items-center gap-4">
        <button
          type="button"
          onClick={() => router.push('/')}
          aria-label={t('common.back')}
          className="-ml-2 rounded-lg p-1.5 text-slate-400 hover:bg-slate-100 hover:text-slate-700"
        >
          <ChevronLeft className="h-5 w-5" />
        </button>

        <div className="min-w-0 flex-1">
          {editing ? (
            <div className="flex items-center gap-1.5">
              <input
                ref={inputRef}
                value={draft}
                disabled={saving}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void save();
                  if (e.key === 'Escape') setEditing(false);
                }}
                aria-label={t('nav.titleLabel')}
                className="h-8 min-w-0 flex-1 rounded-md border border-blue-300 px-2 text-lg font-semibold text-slate-900 focus:outline-none focus:ring-2 focus:ring-blue-500/40"
              />
              <button type="button" onClick={() => void save()} aria-label={t('common.save')} className="rounded-md p-1.5 text-blue-600 hover:bg-blue-50"><Check className="h-4 w-4" /></button>
              <button type="button" onClick={() => setEditing(false)} aria-label={t('common.cancel')} className="rounded-md p-1.5 text-slate-400 hover:bg-slate-100"><X className="h-4 w-4" /></button>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => setEditing(true)}
              className="group flex max-w-full items-center gap-2 rounded-md text-left"
              title={t('common.rename')}
            >
              <h1 className="truncate text-lg font-semibold tracking-tight text-slate-900">{title}</h1>
              <Pencil className="h-3.5 w-3.5 flex-shrink-0 text-slate-300 opacity-0 transition-opacity group-hover:opacity-100" />
            </button>
          )}

          <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-500">
            {dateLabel && (
              <span className="inline-flex items-center gap-1"><CalendarDays className="h-3.5 w-3.5 text-slate-400" />{dateLabel}</span>
            )}
            {durationSeconds !== null && durationSeconds > 0 && (
              <span className="inline-flex items-center gap-1 tabular-nums"><Clock className="h-3.5 w-3.5 text-slate-400" />{formatDuration(durationSeconds)}</span>
            )}
            <span className="inline-flex items-center gap-1 tabular-nums"><FileText className="h-3.5 w-3.5 text-slate-400" />{t('meeting.segments', { count: segmentCount })}</span>
            {hasSummary
              ? <span className="chip bg-violet-50 text-violet-700"><Sparkles className="h-3 w-3" />{t('library.badgeSummary')}</span>
              : segmentCount > 0 && <span className="chip bg-amber-50 text-amber-700">{t('home.recent.needsSummary')}</span>}
            <MeetingLabels meetingId={meetingId} />
          </div>
        </div>

        <div className="flex flex-shrink-0 items-center gap-2">
          <Button variant="ghost" size="sm" onClick={() => router.push(`/notes?id=${encodeURIComponent(meetingId)}`)}>
            <StickyNote />
            <span className="hidden lg:inline">{t('nav.notes')}</span>
          </Button>
          <Button size="sm" onClick={onExport}>
            <Download />
            {t('export.button')}
          </Button>
        </div>
      </div>
    </header>
  );
}
