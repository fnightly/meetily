'use client';

import React, { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Download, FileText, Loader2 } from 'lucide-react';
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
import type { ExportScope } from '@/lib/meeting-export';
import { EXPORT_FORMATS, formatUsesScope, type ExportFormat } from '@/lib/export-formats';
import {
  EXPORT_FORMAT_STORAGE_KEY,
  EXPORT_SCOPE_STORAGE_KEY,
  revealExportPath,
  runExport,
} from '@/lib/export-service';

interface ExportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingIds: string[];
  /** Shown in the description when exporting a single meeting. */
  title?: string;
  /** Unsaved summary from the open editor (single meeting only). */
  summaryOverride?: MeetingSummary | null;
  onExported?: () => void;
}

const SCOPES: ExportScope[] = ['both', 'summary', 'transcript'];

function readStored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  if (typeof window === 'undefined') return fallback;
  const value = window.localStorage.getItem(key) as T | null;
  return value && allowed.includes(value) ? value : fallback;
}

/**
 * One export dialog for a single meeting or a batch: pick the format and what
 * to include, then save. The last choice is remembered.
 */
export function ExportDialog({
  open,
  onOpenChange,
  meetingIds,
  title,
  summaryOverride,
  onExported,
}: ExportDialogProps) {
  const { t, locale } = useI18n();
  const [format, setFormat] = useState<ExportFormat>('docx');
  const [scope, setScope] = useState<ExportScope>('both');
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    setFormat(readStored(EXPORT_FORMAT_STORAGE_KEY, EXPORT_FORMATS, 'docx'));
    setScope(readStored(EXPORT_SCOPE_STORAGE_KEY, SCOPES, 'both'));
    setProgress(null);
  }, [open]);

  const count = meetingIds.length;

  const handleExport = async () => {
    if (count === 0) return;
    setBusy(true);
    window.localStorage.setItem(EXPORT_FORMAT_STORAGE_KEY, format);
    window.localStorage.setItem(EXPORT_SCOPE_STORAGE_KEY, scope);
    try {
      const result = await runExport({
        meetingIds,
        format,
        scope,
        t,
        locale,
        summaryOverride,
        onProgress: (done, total) => setProgress({ done, total }),
      });
      if (result.cancelled) return;
      const target = result.paths.length === 1 ? result.paths[0] : result.folder;
      toast.success(
        result.paths.length === 1
          ? t('export.doneOne')
          : t('export.doneMany', { count: result.paths.length }),
        {
          description:
            (result.failed.length > 0 ? t('export.someFailed', { count: result.failed.length }) + ' ' : '') +
            (target ?? ''),
          action: target
            ? { label: t('export.reveal'), onClick: () => void revealExportPath(target) }
            : undefined,
          duration: 8000,
        },
      );
      onExported?.();
      onOpenChange(false);
    } catch (error) {
      console.error('Export failed:', error);
      toast.error(t('export.failed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Download className="w-5 h-5" />
            {count > 1 ? t('export.titleMany', { count }) : t('export.title')}
          </DialogTitle>
          <DialogDescription>
            {count === 1 && title ? title : t('export.descriptionMany')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div>
            <div className="text-sm font-medium text-gray-900 mb-2">{t('export.format')}</div>
            <div className="grid grid-cols-2 gap-2" role="radiogroup" aria-label={t('export.format')}>
              {EXPORT_FORMATS.map((f) => (
                <button
                  key={f}
                  type="button"
                  role="radio"
                  aria-checked={format === f}
                  onClick={() => setFormat(f)}
                  className={`text-left rounded-lg border px-3 py-2 transition-colors ${
                    format === f
                      ? 'border-blue-500 bg-blue-50 ring-1 ring-blue-500'
                      : 'border-gray-200 hover:bg-gray-50'
                  }`}
                >
                  <div className="flex items-center gap-1.5 text-sm font-medium text-gray-900">
                    <FileText className="w-3.5 h-3.5 text-gray-500" />
                    {t(`export.fmt.${f}`)}
                    <span className="ml-auto text-[10px] font-mono text-gray-400">.{f}</span>
                  </div>
                  <div className="text-xs text-gray-500 mt-0.5">{t(`export.fmtHint.${f}`)}</div>
                </button>
              ))}
            </div>
          </div>

          {formatUsesScope(format) ? (
            <div>
              <div className="text-sm font-medium text-gray-900 mb-2">{t('export.include')}</div>
              <div className="flex gap-2" role="radiogroup" aria-label={t('export.include')}>
                {SCOPES.map((s) => (
                  <button
                    key={s}
                    type="button"
                    role="radio"
                    aria-checked={scope === s}
                    onClick={() => setScope(s)}
                    className={`rounded-full border px-3 py-1 text-sm transition-colors ${
                      scope === s
                        ? 'border-blue-500 bg-blue-50 text-blue-700'
                        : 'border-gray-200 text-gray-700 hover:bg-gray-50'
                    }`}
                  >
                    {t(`export.scope.${s}`)}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <p className="text-xs text-gray-500">{t('export.subtitleNote')}</p>
          )}

          {count > 1 && <p className="text-xs text-gray-500">{t('export.batchNote')}</p>}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            {t('common.cancel')}
          </Button>
          <Button onClick={() => void handleExport()} disabled={busy || count === 0}>
            {busy ? <Loader2 className="w-4 h-4 animate-spin" /> : <Download className="w-4 h-4" />}
            <span className="ml-2">
              {busy && progress && progress.total > 1
                ? t('export.progress', { done: progress.done, total: progress.total })
                : t('export.action')}
            </span>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
