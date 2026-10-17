'use client';

import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  AlertTriangle,
  CheckSquare,
  Download,
  FileAudio,
  FileText,
  FolderInput,
  FolderOpen,
  Loader2,
  Pencil,
  RefreshCw,
  Search,
  Sparkles,
  Square,
  Trash2,
  X,
  Clock,
  HardDrive,
} from 'lucide-react';
import { ExportDialog } from '@/components/ExportDialog';
import {
  groupByDate,
  libraryStats,
  LIBRARY_FILTERS,
  LIBRARY_SORTS,
  matchesFilter,
  matchesQuery,
  meetingIdsByFolder,
  sortRecordings,
  splitHoursMinutes,
  type LibraryFilter,
  type LibrarySort,
} from '@/lib/library-view';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useI18n } from '@/contexts/I18nContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  canSummarise,
  canTranscribe,
  formatBytes,
  formatDuration,
  formatRecordedAt,
  type RecordingEntry,
  type RecordingMutationResult,
} from '@/lib/recordings';

interface RecordingsLibraryProps {
  /** Called after a change that may affect other views. */
  onChanged?: () => void;
}

/**
 * Manage the recordings saved on disk.
 *
 * Reads the recordings folder rather than the meeting database, so a recording
 * whose meeting row was never written — an interrupted session, or a deferred
 * transcription — is still visible and manageable.
 *
 * The folder is read once, by the sidebar provider, and shared: the merged sidebar
 * list is built from the same data, so a delete or rename here is reflected there
 * without a second read of the disk.
 */
export function RecordingsLibrary({ onChanged }: RecordingsLibraryProps) {
  const { t } = useI18n();
  const {
    recordingsView: view,
    recordingsLoading: loading,
    recordingsError,
    refetchRecordings: load,
    entries,
    refetchMeetings,
  } = useSidebar();
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<LibraryFilter>('all');
  const [sort, setSort] = useState<LibrarySort>('newest');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [transcriptHits, setTranscriptHits] = useState<Set<string>>(new Set());
  const [exportIds, setExportIds] = useState<string[] | null>(null);
  const [exportTitle, setExportTitle] = useState<string | undefined>(undefined);
  const [batchDeleteOpen, setBatchDeleteOpen] = useState(false);
  const [deleteMeetingsToo, setDeleteMeetingsToo] = useState(false);
  const [batchBusy, setBatchBusy] = useState(false);

  const folderToMeeting = useMemo(() => meetingIdsByFolder(entries), [entries]);

  // Full-text search over transcripts, debounced; hits are mapped to folders.
  useEffect(() => {
    const q = query.trim();
    if (q.length < 2) {
      setTranscriptHits(new Set());
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(async () => {
      try {
        const results = await invoke<Array<{ id: string }>>('api_search_transcripts', { query: q });
        if (cancelled) return;
        const ids = new Set(results.map((r) => r.id));
        const folders = new Set(
          entries.filter((e) => e.meetingId && e.folderPath && ids.has(e.meetingId)).map((e) => e.folderPath as string),
        );
        // Also keep the raw paths of recordings so the lookup tolerates separators.
        for (const r of view?.recordings ?? []) {
          const id = folderToMeeting.get(r.folder_path);
          if (id && ids.has(id)) folders.add(r.folder_path);
        }
        setTranscriptHits(folders);
      } catch {
        if (!cancelled) setTranscriptHits(new Set());
      }
    }, 300);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, entries, view, folderToMeeting]);

  const visible = useMemo(() => {
    const list = (view?.recordings ?? []).filter(
      (e) => matchesFilter(e, filter) && matchesQuery(e, query, transcriptHits),
    );
    return sortRecordings(list, sort);
  }, [view, filter, query, transcriptHits, sort]);

  const groups = useMemo(
    () => (sort === 'newest' || sort === 'oldest' ? groupByDate(visible) : [{ bucket: null, items: visible }]),
    [visible, sort],
  );
  const stats = useMemo(() => libraryStats(view?.recordings ?? []), [view]);

  // Drop selections that disappeared after a reload.
  useEffect(() => {
    const present = new Set((view?.recordings ?? []).map((r) => r.folder_path));
    setSelected((prev) => {
      const next = new Set([...prev].filter((p) => present.has(p)));
      return next.size === prev.size ? prev : next;
    });
  }, [view]);

  const selectedEntries = (view?.recordings ?? []).filter((r) => selected.has(r.folder_path));
  const exportableIds = selectedEntries
    .map((r) => (r.has_transcript || r.has_summary ? folderToMeeting.get(r.folder_path) : undefined))
    .filter((id): id is string => !!id);
  const allVisibleSelected = visible.length > 0 && visible.every((r) => selected.has(r.folder_path));

  const toggleOne = (folder: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(folder)) next.delete(folder);
      else next.add(folder);
      return next;
    });
  const toggleAllVisible = () =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (allVisibleSelected) visible.forEach((r) => next.delete(r.folder_path));
      else visible.forEach((r) => next.add(r.folder_path));
      return next;
    });

  const formatTotal = (seconds: number) => {
    const { hours, minutes } = splitHoursMinutes(seconds);
    return hours > 0 ? t('library.totalHours', { h: hours, m: minutes }) : t('library.totalMinutes', { m: minutes });
  };

  const handleBatchDelete = async () => {
    setBatchDeleteOpen(false);
    setBatchBusy(true);
    let ok = 0;
    const errors: string[] = [];
    for (const entry of selectedEntries) {
      try {
        await invoke('delete_saved_recording', { folderPath: entry.folder_path });
        const meetingId = folderToMeeting.get(entry.folder_path);
        if (deleteMeetingsToo && meetingId) {
          await invoke('api_delete_meeting', { meetingId });
        }
        ok++;
      } catch (error) {
        errors.push(`${entry.meeting_name}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    setSelected(new Set());
    setBatchBusy(false);
    if (ok > 0) toast.success(t('library.batchDeleted', { count: ok }));
    if (errors.length > 0) toast.error(t('library.operationFailed'), { description: errors.slice(0, 3).join('\n') });
    await load();
    if (deleteMeetingsToo) await refetchMeetings();
    onChanged?.();
  };
  const [busy, setBusy] = useState<string | null>(null);
  const [renameTarget, setRenameTarget] = useState<RecordingEntry | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [deleteTarget, setDeleteTarget] = useState<RecordingEntry | null>(null);

  /**
   * Run an operation that changes a recording, then reload.
   *
   * A rename or move changes folder paths, so the list is re-read rather than
   * patched locally: a stale path would make the next action target the wrong
   * place.
   */
  const runMutation = async (
    folderPath: string,
    command: string,
    args: Record<string, unknown>,
    successMessage: (result: RecordingMutationResult) => string,
    refresh = true,
  ) => {
    setBusy(folderPath);
    try {
      const result = await invoke<RecordingMutationResult>(command, {
        folderPath,
        ...args,
      });
      toast.success(successMessage(result));
      if (refresh) await load();
      onChanged?.();
      return result;
    } catch (error) {
      console.error(`${command} failed:`, error);
      toast.error(t('library.operationFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
      return null;
    } finally {
      setBusy(null);
    }
  };

  const handleOpen = (entry: RecordingEntry) =>
    runMutation(
      entry.folder_path,
      'open_recording_folder',
      {},
      () => 'Opened the recording folder',
      false,
    );

  const handleMove = (entry: RecordingEntry) =>
    runMutation(
      entry.folder_path,
      'move_saved_recording',
      {},
      (result) => result.detail,
    );

  const handleDelete = async () => {
    const entry = deleteTarget;
    setDeleteTarget(null);
    if (!entry) return;
    await runMutation(entry.folder_path, 'delete_saved_recording', {}, (r) => r.detail);
  };

  const handleRenameConfirm = async () => {
    const entry = renameTarget;
    const title = renameValue.trim();
    setRenameTarget(null);
    if (!entry || !title) return;
    await runMutation(entry.folder_path, 'rename_saved_recording', { newTitle: title }, (r) =>
      `Renamed to “${r.title}”`,
    );
  };

  const handleTranscribe = async (entry: RecordingEntry) => {
    setBusy(entry.folder_path);
    try {
      // Retranscription attaches transcripts to a meeting row, so a recording
      // without one is saved first.
      const saved = await invoke<{ meeting_id?: string }>('api_save_transcript', {
        meetingTitle: entry.meeting_name,
        transcripts: [],
        folderPath: entry.folder_path,
      });

      const meetingId = saved?.meeting_id;
      if (!meetingId) throw new Error('No meeting id was returned');

      await invoke('start_retranscription_command', {
        meetingId,
        meetingFolderPath: entry.folder_path,
        language: null,
        model: null,
        provider: null,
      });

      toast.info(t('library.transcribeStarted'), {
        description: t('library.transcribeStartedDetail'),
      });
      await load();
      onChanged?.();
    } catch (error) {
      console.error('Failed to start transcription:', error);
      toast.error(t('library.transcribeFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(null);
    }
  };

  const handleSummarise = (entry: RecordingEntry) => {
    // Summarising is owned by the meeting page, which holds the model
    // configuration and builds the transcript payload.
    const params = new URLSearchParams({
      source: 'recording',
      generateSummary: '1',
      folderPath: entry.folder_path,
    });
    window.location.href = `/meeting-details?${params.toString()}`;
  };

  const handleRemoveSummary = (entry: RecordingEntry) =>
    runMutation(
      entry.folder_path,
      'delete_saved_recording_summary',
      {},
      (r) => r.detail,
    );

  if (loading && !view) {
    return (
      <div className="flex items-center gap-2 text-sm text-gray-500 p-4">
        <Loader2 className="w-4 h-4 animate-spin" />
        {t('library.loading')}
      </div>
    );
  }

  // A failed read is shown with a way to retry rather than as an empty library:
  // "nothing here" and "could not look" mean very different things.
  if (recordingsError && !view) {
    return (
      <div
        role="alert"
        className="flex items-center justify-between gap-4 p-4 bg-red-50 border border-red-200 rounded-lg text-sm text-red-800"
      >
        <span>
          {t('library.loadFailed')}
          {': '}
          {recordingsError}
        </span>
        <Button variant="outline" size="sm" onClick={() => void load()}>
          <RefreshCw className="w-4 h-4" />
          <span className="ml-2">{t('common.retry')}</span>
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {stats.count > 0 && (
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-3" aria-label={t('library.statsLabel')}>
          {([
            [t('library.statCount'), String(stats.count), FileAudio, 'bg-blue-50 text-blue-600'],
            [t('library.statDuration'), formatTotal(stats.totalSeconds), Clock, 'bg-sky-50 text-sky-600'],
            [t('library.statSize'), formatBytes(stats.totalBytes), HardDrive, 'bg-slate-100 text-slate-600'],
            [t('library.statDone'), t('library.statDoneValue', { tr: stats.transcribed, sum: stats.summarized }), Sparkles, 'bg-violet-50 text-violet-600'],
          ] as const).map(([label, value, Icon, tone]) => (
            <div key={label} className="surface-card flex items-center gap-3 px-4 py-3">
              <span className={`flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg ${tone}`}>
                <Icon className="w-4 h-4" />
              </span>
              <div className="min-w-0">
                <div className="text-lg font-semibold leading-6 text-slate-900 truncate tabular-nums">{value}</div>
                <div className="text-xs text-slate-500">{label}</div>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="flex items-center gap-2 text-xs text-slate-500 min-w-0">
        <FolderOpen className="w-3.5 h-3.5 flex-shrink-0 text-slate-400" />
        <span className="flex-shrink-0">{t('library.folder')}:</span>
        <span className="font-mono truncate" title={view?.folder ?? ''}>{view?.folder ?? '—'}</span>
        <Button variant="ghost" size="sm" className="ml-auto h-7 px-2 text-slate-500" onClick={() => void load()} disabled={loading}>
          {loading ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <RefreshCw className="w-3.5 h-3.5" />}
          <span>{t('common.refresh')}</span>
        </Button>
      </div>

      {view?.folder_missing && (
        <div className="p-3 bg-amber-50 border border-amber-200 rounded-lg text-xs text-amber-900">
          {t('library.folderMissing')}
        </div>
      )}

      {view && view.recordings.length === 0 && !view.folder_missing && (
        <div className="surface-card p-10 text-center text-sm text-slate-500">
          {t('library.empty')}
        </div>
      )}

      {view && view.recordings.length > 0 && (
        <div className="surface-card space-y-3 sticky top-0 z-10 p-3">
          <div className="flex items-center gap-2 flex-wrap">
            <div className="relative flex-1 min-w-[200px]">
              <Search className="w-4 h-4 text-gray-400 absolute left-2.5 top-1/2 -translate-y-1/2" />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={t('library.searchPlaceholder')}
                className="pl-8 pr-8 h-9 bg-slate-50 border-slate-200"
                aria-label={t('library.searchPlaceholder')}
              />
              {query && (
                <button
                  type="button"
                  className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600"
                  onClick={() => setQuery('')}
                  aria-label={t('common.clear')}
                >
                  <X className="w-4 h-4" />
                </button>
              )}
            </div>
            <label className="flex items-center gap-1.5 text-sm text-gray-600">
              <span className="whitespace-nowrap">{t('library.sortLabel')}</span>
              <select
                value={sort}
                onChange={(e) => setSort(e.target.value as LibrarySort)}
                className="h-9 rounded-lg border border-slate-200 bg-white px-2 text-sm"
              >
                {LIBRARY_SORTS.map((s) => (
                  <option key={s} value={s}>
                    {t(`library.sort.${s}`)}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="flex items-center gap-1.5 flex-wrap" role="radiogroup" aria-label={t('library.filterLabel')}>
            {LIBRARY_FILTERS.map((f) => {
              const n = (view?.recordings ?? []).filter((e) => matchesFilter(e, f)).length;
              return (
                <button
                  key={f}
                  type="button"
                  role="radio"
                  aria-checked={filter === f}
                  onClick={() => setFilter(f)}
                  className={`rounded-full px-3 py-1 text-xs font-medium transition-colors ${
                    filter === f
                      ? 'bg-slate-900 text-white'
                      : 'bg-slate-100 text-slate-600 hover:bg-slate-200'
                  }`}
                >
                  {t(`library.filter.${f}`)} <span className={filter === f ? 'text-white/60' : 'text-slate-400'}>{n}</span>
                </button>
              );
            })}
          </div>

          <div className="flex items-center gap-2 text-sm border-t border-slate-100 pt-2.5 min-h-[36px]">
            <button
              type="button"
              onClick={toggleAllVisible}
              className="inline-flex items-center gap-1.5 text-gray-600 hover:text-gray-900"
              disabled={visible.length === 0}
            >
              {allVisibleSelected ? <CheckSquare className="w-4 h-4 text-blue-600" /> : <Square className="w-4 h-4" />}
              {t('library.selectAll')}
            </button>
            {selected.size > 0 ? (
              <>
                <span className="text-gray-500">{t('library.selectedCount', { count: selected.size })}</span>
                <div className="ml-auto flex items-center gap-1.5">
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={batchBusy || exportableIds.length === 0}
                    title={exportableIds.length < selected.size ? t('library.exportPartialHint') : undefined}
                    onClick={() => {
                      setExportTitle(undefined);
                      setExportIds(exportableIds);
                    }}
                  >
                    <Download className="w-4 h-4" />
                    <span className="ml-1.5">{t('library.exportSelected', { count: exportableIds.length })}</span>
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    className="text-red-600 hover:text-red-700"
                    disabled={batchBusy}
                    onClick={() => {
                      setDeleteMeetingsToo(false);
                      setBatchDeleteOpen(true);
                    }}
                  >
                    {batchBusy ? <Loader2 className="w-4 h-4 animate-spin" /> : <Trash2 className="w-4 h-4" />}
                    <span className="ml-1.5">{t('library.deleteSelected')}</span>
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
                    {t('library.clearSelection')}
                  </Button>
                </div>
              </>
            ) : (
              <span className="text-gray-400 text-xs">
                {visible.length !== (view?.recordings.length ?? 0)
                  ? t('library.showing', { shown: visible.length, total: view?.recordings.length ?? 0 })
                  : t('library.selectHint')}
              </span>
            )}
          </div>
        </div>
      )}

      {view && view.recordings.length > 0 && visible.length === 0 && (
        <div className="p-6 border border-dashed rounded-lg text-center text-sm text-gray-500">
          {t('library.noMatches')}
        </div>
      )}

      <div className="space-y-4">
        {groups.map((group) => (
          <section key={group.bucket ?? 'all'} className="space-y-2">
            {group.bucket && (
              <h5 className="eyebrow px-1">
                {t(`library.bucket.${group.bucket}`)} · {group.items.length}
              </h5>
            )}
        {group.items.map((entry) => {
          const isBusy = busy === entry.folder_path || batchBusy;
          const isSelected = selected.has(entry.folder_path);
          const meetingId = folderToMeeting.get(entry.folder_path);

          return (
            <div
              key={entry.folder_path}
              className={`group surface-card px-4 py-3 flex items-start gap-3 transition-colors ${
                isSelected ? '!border-blue-300 bg-blue-50/50' : 'hover:border-slate-300'
              }`}
            >
              <button
                type="button"
                role="checkbox"
                aria-checked={isSelected}
                aria-label={t('library.selectOne', { name: entry.meeting_name })}
                onClick={() => toggleOne(entry.folder_path)}
                className="mt-2 shrink-0 text-slate-300 hover:text-slate-600"
              >
                {isSelected ? <CheckSquare className="w-5 h-5 text-blue-600" /> : <Square className="w-5 h-5" />}
              </button>
              <span className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg ${entry.has_summary ? 'bg-violet-50 text-violet-600' : entry.has_transcript ? 'bg-blue-50 text-blue-600' : 'bg-slate-100 text-slate-500'}`}>
                <FileAudio className="w-4 h-4" />
              </span>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="font-semibold text-sm text-slate-900 truncate">{entry.meeting_name}</span>

                  {!entry.audio_file && (
                    <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-red-100 text-red-800">
                      {t('library.badgeNoAudio')}
                    </span>
                  )}
                  {entry.status === 'recording' && (
                    <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-amber-100 text-amber-800">
                      {t('library.badgeIncomplete')}
                    </span>
                  )}
                  {entry.has_transcript && (
                    <span className="chip bg-blue-50 text-blue-700">
                      <FileText className="w-3 h-3" />
                      {t('library.badgeTranscript')}
                    </span>
                  )}
                  {entry.has_summary && (
                    <span className="chip bg-violet-50 text-violet-700">
                      <Sparkles className="w-3 h-3" />
                      {t('library.badgeSummary')}
                    </span>
                  )}
                </div>

                <div className="text-xs text-slate-500 mt-1 flex items-center gap-1.5 flex-wrap tabular-nums">
                  <span>{formatRecordedAt(entry.created_at)}</span>
                  <span>·</span>
                  <span>{formatDuration(entry.duration_seconds)}</span>
                  <span>·</span>
                  <span>{formatBytes(entry.audio_bytes)}</span>
                  {entry.audio_file && (
                    <>
                      <span>·</span>
                      <span className="font-mono text-slate-400">{entry.audio_file}</span>
                    </>
                  )}
                </div>

                {!entry.audio_file && (
                  <div className="text-xs text-amber-800 mt-1 flex items-center gap-1">
                    <AlertTriangle className="w-3 h-3" />
                    {t('library.noAudioWarning')}
                  </div>
                )}
              </div>

              <div className="flex items-center gap-1 shrink-0 flex-wrap justify-end">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => void handleOpen(entry)}
                  disabled={isBusy}
                  title={t('library.openFolder')}
                >
                  <FolderOpen className="w-4 h-4" />
                </Button>

                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setRenameTarget(entry);
                    setRenameValue(entry.meeting_name);
                  }}
                  disabled={isBusy}
                  title={t('library.renameTitle')}
                >
                  <Pencil className="w-4 h-4" />
                </Button>

                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => void handleMove(entry)}
                  disabled={isBusy}
                  title={t('library.moveTitle')}
                >
                  <FolderInput className="w-4 h-4" />
                </Button>

                {canTranscribe(entry) && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void handleTranscribe(entry)}
                    disabled={isBusy}
                    title={t('library.transcribeTitle')}
                  >
                    {isBusy ? (
                      <Loader2 className="w-4 h-4 animate-spin" />
                    ) : (
                      <FileText className="w-4 h-4" />
                    )}
                    <span className="ml-1.5">{t('library.transcribe')}</span>
                  </Button>
                )}

                {canSummarise(entry) && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleSummarise(entry)}
                    disabled={isBusy}
                    title={t('library.summariseTitle')}
                  >
                    <Sparkles className="w-4 h-4" />
                    <span className="ml-1.5">
                      {entry.has_summary ? t('library.resummarise') : t('library.summarise')}
                    </span>
                  </Button>
                )}

                {entry.has_summary && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void handleRemoveSummary(entry)}
                    disabled={isBusy}
                    title={t('library.removeSummaryTitle')}
                  >
                    {t('library.removeSummary')}
                  </Button>
                )}

                {meetingId && (entry.has_transcript || entry.has_summary) && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      setExportTitle(entry.meeting_name);
                      setExportIds([meetingId]);
                    }}
                    disabled={isBusy}
                    title={t('library.exportTitle')}
                  >
                    <Download className="w-4 h-4" />
                  </Button>
                )}

                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setDeleteTarget(entry)}
                  disabled={isBusy}
                  className="text-red-600 hover:text-red-700"
                  title={t('library.deleteTitle')}
                >
                  <Trash2 className="w-4 h-4" />
                </Button>
              </div>
            </div>
          );
        })}
          </section>
        ))}
      </div>

      <p className="text-xs text-gray-500">
        {t('library.footnote')}
      </p>

      {/* Rename */}
      <Dialog open={renameTarget !== null} onOpenChange={(open) => !open && setRenameTarget(null)}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t('library.renameDialogTitle')}</DialogTitle>
          </DialogHeader>
          <Input
            value={renameValue}
            onChange={(event) => setRenameValue(event.target.value)}
            placeholder={t('library.namePlaceholder')}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void handleRenameConfirm();
            }}
          />
          <DialogFooter>
            <Button variant="ghost" onClick={() => setRenameTarget(null)}>
              {t('common.cancel')}
            </Button>
            <Button onClick={() => void handleRenameConfirm()} disabled={!renameValue.trim()}>
              {t('common.rename')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete confirmation */}
      <Dialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t('library.deleteDialogTitle')}</DialogTitle>
          </DialogHeader>
          <p className="text-sm text-gray-600">
            {t('library.deleteDialogBody', { name: deleteTarget?.meeting_name ?? '' })}
          </p>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setDeleteTarget(null)}>
              {t('common.cancel')}
            </Button>
            <Button variant="destructive" onClick={() => void handleDelete()}>
              {t('common.delete')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {/* Batch delete confirmation */}
      <Dialog open={batchDeleteOpen} onOpenChange={setBatchDeleteOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t('library.batchDeleteTitle', { count: selected.size })}</DialogTitle>
          </DialogHeader>
          <p className="text-sm text-gray-600">{t('library.batchDeleteBody')}</p>
          <label className="flex items-start gap-2 text-sm text-gray-700 cursor-pointer">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={deleteMeetingsToo}
              onChange={(e) => setDeleteMeetingsToo(e.target.checked)}
            />
            <span>{t('library.batchDeleteMeetings')}</span>
          </label>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setBatchDeleteOpen(false)}>
              {t('common.cancel')}
            </Button>
            <Button variant="destructive" onClick={() => void handleBatchDelete()}>
              {t('common.delete')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ExportDialog
        open={exportIds !== null}
        onOpenChange={(open) => !open && setExportIds(null)}
        meetingIds={exportIds ?? []}
        title={exportTitle}
      />
    </div>
  );
}
