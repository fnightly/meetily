'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  AlertTriangle,
  FileAudio,
  FileText,
  FolderInput,
  FolderOpen,
  Loader2,
  Pencil,
  RefreshCw,
  Sparkles,
  Trash2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useI18n } from '@/contexts/I18nContext';
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
  type RecordingsListView,
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
 */
export function RecordingsLibrary({ onChanged }: RecordingsLibraryProps) {
  const { t } = useI18n();
  const [view, setView] = useState<RecordingsListView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [renameTarget, setRenameTarget] = useState<RecordingEntry | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [deleteTarget, setDeleteTarget] = useState<RecordingEntry | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setView(await invoke<RecordingsListView>('list_saved_recordings'));
    } catch (error) {
      console.error('Failed to list saved recordings:', error);
      toast.error(t('library.loadFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

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
        Reading the recordings folder…
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h4 className="text-base font-medium text-gray-900">{t('library.title')}</h4>
          <p className="text-sm text-gray-600 mt-1">
            {t('library.description')}
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
          {loading ? (
            <Loader2 className="w-4 h-4 animate-spin" />
          ) : (
            <RefreshCw className="w-4 h-4" />
          )}
          <span className="ml-2">{t('common.refresh')}</span>
        </Button>
      </div>

      <div className="p-3 bg-gray-50 border rounded-lg text-xs text-gray-600 break-all">
        <span className="font-medium">{t('library.folder')}:</span> {view?.folder ?? '—'}
        {view && view.recordings.length > 0 && (
          <span className="ml-2">
            PLACEHOLDER_SKIP
            {view.recordings.length === 1 ? '' : 's'} · {formatBytes(view.total_bytes)} of audio
          </span>
        )}
      </div>

      {view?.folder_missing && (
        <div className="p-3 bg-amber-50 border border-amber-200 rounded-lg text-xs text-amber-900">
          {t('library.folderMissing')}
        </div>
      )}

      {view && view.recordings.length === 0 && !view.folder_missing && (
        <div className="p-6 border rounded-lg text-center text-sm text-gray-500">
          {t('library.empty')}
        </div>
      )}

      <div className="space-y-2">
        {view?.recordings.map((entry) => {
          const isBusy = busy === entry.folder_path;

          return (
            <div
              key={entry.folder_path}
              className="p-3 border rounded-lg flex items-start gap-3 hover:bg-gray-50 transition-colors"
            >
              <FileAudio className="w-5 h-5 text-gray-400 mt-0.5 shrink-0" />

              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="font-medium text-sm truncate">{entry.meeting_name}</span>

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
                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-blue-100 text-blue-800 inline-flex items-center gap-1">
                      <FileText className="w-3 h-3" />
                      {t('library.badgeTranscript')}
                    </span>
                  )}
                  {entry.has_summary && (
                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-purple-100 text-purple-800 inline-flex items-center gap-1">
                      <Sparkles className="w-3 h-3" />
                      {t('library.badgeSummary')}
                    </span>
                  )}
                </div>

                <div className="text-xs text-gray-500 mt-1 flex items-center gap-2 flex-wrap">
                  <span>{formatRecordedAt(entry.created_at)}</span>
                  <span>·</span>
                  <span>{formatDuration(entry.duration_seconds)}</span>
                  <span>·</span>
                  <span>{formatBytes(entry.audio_bytes)}</span>
                  {entry.audio_file && (
                    <>
                      <span>·</span>
                      <span className="font-mono">{entry.audio_file}</span>
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
    </div>
  );
}
