'use client';

import { useEffect, useState } from 'react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import {
  AlertTriangle,
  CheckCircle2,
  Database,
  FileText,
  Loader2,
  Share2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { useI18n } from '@/contexts/I18nContext';
import type { MeetingSummary } from '@/types';
import { revealExportPath } from '@/lib/export-service';
import {
  getSyncSettings,
  prepareSyncNote,
  syncMeetingToNotion,
  syncMeetingToObsidian,
  type NotionSyncResult,
  type ObsidianSyncResult,
  type PreparedSyncNote,
  type SyncSettingsView,
} from '@/lib/sync';

interface SyncDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  /** Shown in the description; falls back to the note's own title. */
  title?: string;
  /** Unsaved summary from the open editor, so the note matches the screen. */
  summaryOverride?: MeetingSummary | null;
}

type Target = 'obsidian' | 'notion';

type SyncResult =
  | { target: 'obsidian'; obsidian: ObsidianSyncResult }
  | { target: 'notion'; notion: NotionSyncResult };

type SyncError = { target: Target; message: string };

/**
 * Explicit one-meeting sync.
 *
 * Nothing is sent when the dialog opens: settings and the note are read locally,
 * and the network is only touched after a target's Sync button is pressed. That
 * separation is the point of the dialog — a meeting must never leave the machine
 * as a side effect of opening a screen.
 */
export function SyncDialog({
  open,
  onOpenChange,
  meetingId,
  title,
  summaryOverride,
}: SyncDialogProps) {
  const { t } = useI18n();
  const router = useRouter();

  const [view, setView] = useState<SyncSettingsView | null>(null);
  const [note, setNote] = useState<PreparedSyncNote | null>(null);
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState<Target | null>(null);
  const [error, setError] = useState<SyncError | string | null>(null);
  const [result, setResult] = useState<SyncResult | null>(null);
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    if (!open) return;

    let cancelled = false;
    setLoading(true);
    setError(null);
    setResult(null);
    setSyncing(null);
    setView(null);
    setNote(null);
    setRevealed(false);

    void (async () => {
      try {
        const [settings, prepared] = await Promise.all([
          getSyncSettings(),
          prepareSyncNote(meetingId, t, summaryOverride),
        ]);
        if (cancelled) return;
        setView(settings);
        setNote(prepared);
      } catch (cause) {
        if (cancelled) return;
        setError(`${t('sync.meetingLoadFailed')}: ${cause instanceof Error ? cause.message : String(cause)}`);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [open, meetingId, summaryOverride, t]);

  const handleSync = async (target: Target) => {
    if (!note) return;

    setSyncing(target);
    setError(null);
    setResult(null);
    setRevealed(false);

    try {
      if (target === 'obsidian') {
        const outcome = await syncMeetingToObsidian(note.payload);
        setResult({ target: 'obsidian', obsidian: outcome });
      } else {
        const outcome = await syncMeetingToNotion(note.payload);
        setResult({ target: 'notion', notion: outcome });
      }
    } catch (cause) {
      setError({
        target,
        message: cause instanceof Error ? cause.message : String(cause),
      });
    } finally {
      setSyncing(null);
    }
  };

  const obsidianConfigured = Boolean(view?.effective_obsidian_folder?.trim());
  const notionConfigured = Boolean(
    view?.notion_token_set && view.settings.notion_database_id?.trim(),
  );
  const busy = loading || syncing !== null;
  const errorTarget = typeof error === 'object' && error !== null ? error.target : null;
  const errorMessage =
    typeof error === 'string' ? error : error?.message ?? null;

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Share2 className="w-5 h-5" />
            {t('sync.title')}
          </DialogTitle>
          <DialogDescription>
            {title ? `${title} — ${t('sync.description')}` : t('sync.description')}
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 p-4 text-sm text-slate-600">
            <Loader2 className="w-4 h-4 animate-spin" />
            {t('sync.loading')}
          </div>
        ) : note && !note.hasSummary ? (
          <div className="flex gap-2 rounded-md border border-amber-200 bg-amber-50 p-3">
            <AlertTriangle className="mt-0.5 w-4 h-4 shrink-0 text-amber-600" />
            <div>
              <p className="text-sm font-medium text-amber-900">{t('sync.noSummary')}</p>
              <p className="mt-1 text-xs text-amber-800">{t('sync.noSummaryHint')}</p>
            </div>
          </div>
        ) : !note ? (
          <div role="alert" className="flex gap-2 rounded-md border border-red-200 bg-red-50 p-3">
            <AlertTriangle className="mt-0.5 w-4 h-4 shrink-0 text-red-600" />
            <p className="break-words text-xs text-red-800">
              {typeof error === 'string' ? error : t('sync.meetingLoadFailed')}
            </p>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-2">
              {/* Obsidian */}
              <section className="rounded-lg border border-slate-200 bg-slate-50/70 p-4">
                <h4 className="flex items-center gap-2 text-sm font-semibold text-slate-900">
                  <FileText className="w-4 h-4 text-slate-500" />
                  {t('sync.target.obsidian')}
                </h4>
                <p className="mt-1 min-h-[2.5rem] break-all text-xs text-slate-500">
                  {obsidianConfigured
                    ? t('sync.target.obsidianDesc', { path: view?.obsidian_notes_folder ?? '' })
                    : t('sync.obsidianNotConfigured')}
                </p>
                {obsidianConfigured ? (
                  <Button
                    size="sm"
                    className="mt-3 w-full"
                    onClick={() => void handleSync('obsidian')}
                    disabled={busy}
                  >
                    {syncing === 'obsidian' ? (
                      <Loader2 className="w-4 h-4 animate-spin" />
                    ) : (
                      <FileText className="w-4 h-4" />
                    )}
                    <span className="ml-2">
                      {syncing === 'obsidian' ? t('sync.syncing') : t('sync.action')}
                    </span>
                  </Button>
                ) : (
                  <Button
                    variant="outline"
                    size="sm"
                    className="mt-3 w-full"
                    onClick={() => router.push('/settings?tab=sync')}
                  >
                    {t('sync.openSettings')}
                  </Button>
                )}
              </section>

              {/* Notion */}
              <section className="rounded-lg border border-slate-200 bg-slate-50/70 p-4">
                <h4 className="flex items-center gap-2 text-sm font-semibold text-slate-900">
                  <Database className="w-4 h-4 text-slate-500" />
                  {t('sync.target.notion')}
                </h4>
                <p className="mt-1 min-h-[2.5rem] break-all text-xs text-slate-500">
                  {notionConfigured
                    ? t('sync.target.notionDesc', {
                        id: view?.settings.notion_database_id ?? '',
                      })
                    : t('sync.notionNotConfigured')}
                </p>
                {notionConfigured ? (
                  <Button
                    size="sm"
                    className="mt-3 w-full"
                    onClick={() => void handleSync('notion')}
                    disabled={busy}
                  >
                    {syncing === 'notion' ? (
                      <Loader2 className="w-4 h-4 animate-spin" />
                    ) : (
                      <Database className="w-4 h-4" />
                    )}
                    <span className="ml-2">
                      {syncing === 'notion' ? t('sync.syncing') : t('sync.action')}
                    </span>
                  </Button>
                ) : (
                  <Button
                    variant="outline"
                    size="sm"
                    className="mt-3 w-full"
                    onClick={() => router.push('/settings?tab=sync')}
                  >
                    {t('sync.openSettings')}
                  </Button>
                )}
              </section>
            </div>

            {errorMessage && (
              <div role="alert" className="flex gap-2 rounded-md border border-red-200 bg-red-50 p-3">
                <AlertTriangle className="mt-0.5 w-4 h-4 shrink-0 text-red-600" />
                <div>
                  <p className="text-sm font-medium text-red-900">
                    {errorTarget === 'notion'
                      ? t('sync.notionFailed')
                      : t('sync.obsidianFailed')}
                  </p>
                  <p className="mt-1 break-words text-xs text-red-800">{errorMessage}</p>
                </div>
              </div>
            )}

            {result && (
              <div role="status" className="space-y-2 rounded-md border border-green-200 bg-green-50 p-3">
                <div className="flex gap-2">
                  <CheckCircle2 className="mt-0.5 w-4 h-4 shrink-0 text-green-600" />
                  <div className="min-w-0">
                    <p className="text-sm font-medium text-green-900">
                      {result.target === 'obsidian'
                        ? result.obsidian.status === 'unchanged'
                          ? t('sync.obsidianUnchanged')
                          : result.obsidian.status === 'alternative'
                            ? t('sync.obsidianAlternative')
                            : t('sync.obsidianDone')
                        : t('sync.notionDone')}
                    </p>
                    <p className="mt-1 break-all text-xs text-green-800">
                      {result.target === 'obsidian'
                        ? result.obsidian.status === 'unchanged'
                          ? t('sync.obsidianUnchangedDetail', { path: result.obsidian.path })
                          : result.obsidian.status === 'alternative'
                            ? t('sync.obsidianAlternativeDetail', {
                                existing: result.obsidian.existing_path ?? '',
                                path: result.obsidian.path,
                              })
                            : t('sync.obsidianWritten', { path: result.obsidian.path })
                        : result.notion.page_url
                          ? t('sync.notionDoneDetail', {
                              blocks: result.notion.blocks_written,
                              url: result.notion.page_url,
                            })
                          : t('sync.notionDoneNoUrl', {
                              id: result.notion.page_id,
                              blocks: result.notion.blocks_written,
                            })}
                    </p>
                  </div>
                </div>

                <div className="flex justify-end">
                  {result.target === 'obsidian' ? (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={revealed}
                      onClick={() => {
                        setRevealed(true);
                        void invoke('reveal_export_path', { path: result.obsidian.path }).catch(
                          () => setRevealed(false),
                        );
                      }}
                    >
                      {t('export.reveal')}
                    </Button>
                  ) : result.notion.page_url ? (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        void invoke('open_external_url', { url: result.notion.page_url }).catch(
                          (error) => console.error('Failed to open the synced page:', error),
                        )
                      }
                    >
                      {t('sync.openPage')}
                    </Button>
                  ) : null}
                </div>
              </div>
            )}

            <p className="text-xs text-slate-500">{t('sync.privacy')}</p>
          </div>
        )}

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            {t('sync.close')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
