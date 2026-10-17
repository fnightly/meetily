'use client';

import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Archive, ArchiveRestore, Loader2, RotateCcw, ShieldCheck } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { useI18n } from '@/contexts/I18nContext';

interface BackupResult {
  cancelled: boolean;
  path: string | null;
  bytes: number;
  meetings: number;
  media_folders: number;
}

interface RestoreResult {
  cancelled: boolean;
  meetings: number;
  created_at: string | null;
  requires_restart: boolean;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** Whole-library backup to one ZIP, and restore from it (applied on restart). */
export function BackupSettings() {
  const { t, locale } = useI18n();
  const [includeAudio, setIncludeAudio] = useState(false);
  const [busy, setBusy] = useState<'backup' | 'restore' | null>(null);
  const [restored, setRestored] = useState<RestoreResult | null>(null);

  const backup = async () => {
    setBusy('backup');
    try {
      const result = await invoke<BackupResult>('api_create_backup', { includeAudio });
      if (result.cancelled) return;
      toast.success(t('backup.done'), {
        description: t('backup.doneDesc', { count: result.meetings, size: formatBytes(result.bytes), path: result.path ?? '' }),
      });
    } catch (e) {
      toast.error(t('backup.failed'), { description: String(e) });
    } finally {
      setBusy(null);
    }
  };

  const restore = async () => {
    setBusy('restore');
    try {
      const result = await invoke<RestoreResult>('api_restore_backup');
      if (result.cancelled) return;
      setRestored(result);
    } catch (e) {
      toast.error(t('backup.restoreFailed'), { description: String(e) });
    } finally {
      setBusy(null);
    }
  };

  const relaunchNow = async () => {
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    } catch (e) {
      toast.error(t('backup.relaunchFailed'), { description: String(e) });
    }
  };

  const restoredDate = restored?.created_at ? new Date(restored.created_at) : null;

  return (
    <div className="space-y-4">
      <section className="surface-card p-5">
        <div className="flex items-start gap-3">
          <span className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-blue-50 text-blue-600"><Archive className="h-4 w-4" /></span>
          <div className="min-w-0 flex-1">
            <div className="font-semibold text-slate-900">{t('backup.title')}</div>
            <p className="mt-1 text-sm text-slate-500">{t('backup.desc')}</p>
            <label className="mt-4 flex items-center justify-between gap-4 rounded-lg bg-slate-50 px-3 py-2.5">
              <span>
                <span className="block text-sm font-medium text-slate-800">{t('backup.includeAudio')}</span>
                <span className="block text-xs text-slate-500">{t('backup.includeAudioHint')}</span>
              </span>
              <Switch checked={includeAudio} onCheckedChange={setIncludeAudio} aria-label={t('backup.includeAudio')} />
            </label>
            <div className="mt-4 flex items-center gap-3">
              <Button onClick={() => void backup()} disabled={busy !== null}>
                {busy === 'backup' ? <Loader2 className="animate-spin" /> : <Archive />}
                {busy === 'backup' ? t('backup.working') : t('backup.create')}
              </Button>
              <span className="inline-flex items-center gap-1 text-xs text-slate-400"><ShieldCheck className="h-3.5 w-3.5" />{t('backup.localOnly')}</span>
            </div>
          </div>
        </div>
      </section>

      <section className="surface-card p-5">
        <div className="flex items-start gap-3">
          <span className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-amber-50 text-amber-600"><ArchiveRestore className="h-4 w-4" /></span>
          <div className="min-w-0 flex-1">
            <div className="font-semibold text-slate-900">{t('backup.restoreTitle')}</div>
            <p className="mt-1 text-sm text-slate-500">{t('backup.restoreDesc')}</p>
            <Button className="mt-4" variant="outline" onClick={() => void restore()} disabled={busy !== null}>
              {busy === 'restore' ? <Loader2 className="animate-spin" /> : <ArchiveRestore />}
              {t('backup.restore')}
            </Button>
          </div>
        </div>
      </section>

      <Dialog open={!!restored} onOpenChange={(open) => { if (!open) setRestored(null); }}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t('backup.readyTitle')}</DialogTitle>
            <DialogDescription>
              {t('backup.readyDesc', {
                count: restored?.meetings ?? 0,
                date: restoredDate && !Number.isNaN(restoredDate.getTime()) ? restoredDate.toLocaleString(locale) : '—',
              })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setRestored(null)}>{t('backup.later')}</Button>
            <Button onClick={() => void relaunchNow()}><RotateCcw />{t('backup.relaunch')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
