'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { AlertTriangle, CheckCircle2, Download, FolderOpen, Loader2, RefreshCw, Users } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';
import { SpeakerSeparationTuning } from './SpeakerSeparationTuning';

interface DiarizationModelStatus {
  ready: boolean;
  folder: string;
  missing: string[];
  message: string;
  required_bytes: number;
  available_bytes: number | null;
  has_room: boolean;
}

/**
 * Speaker separation model management.
 *
 * The models are ~45 MB, most of it the speaker-embedding model, so they are downloaded
 * on first use rather than bundled. This surface exists because that download has to be
 * something the user initiates and can see the state of — a feature that silently needs
 * 45 MB before it will work is worse than one that says so.
 *
 * Running separation over a meeting is deliberately not here: it belongs beside the
 * transcript it labels, not in settings.
 */
export function SpeakerSeparationSettings() {
  const { t } = useI18n();
  const [status, setStatus] = useState<DiarizationModelStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [downloading, setDownloading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setStatus(await invoke<DiarizationModelStatus>('get_diarization_model_status'));
    } catch (error) {
      console.error('Failed to check speaker separation models:', error);
      toast.error(t('speakers.checkFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleDownload = async () => {
    setDownloading(true);
    try {
      // The command returns the resulting status, so the display updates from the same
      // shape it was already rendering.
      const next = await invoke<DiarizationModelStatus>('download_diarization_models');
      setStatus(next);
      if (next.ready) toast.success(t('speakers.downloadDone'));
    } catch (error) {
      console.error('Failed to download speaker separation models:', error);
      toast.error(t('speakers.downloadFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
      // Status may have changed part way through a failed download.
      await refresh();
    } finally {
      setDownloading(false);
    }
  };

  const handleOpenFolder = async () => {
    try {
      // Creates the folder if it does not exist yet, which is the case before the first
      // download.
      await invoke('open_diarization_models_folder');
    } catch (error) {
      console.error('Failed to open the models folder:', error);
      // Still tell the user where it is, so the path is not lost.
      if (status) toast.info(status.folder);
    }
  };

  const requiredMb = status ? Math.ceil(status.required_bytes / 1_048_576) : 0;

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <Users className="w-4 h-4 text-gray-600" />
            <h4 className="text-base font-medium text-gray-900">{t('speakers.title')}</h4>
          </div>
          <p className="text-sm text-gray-600 mt-1">{t('speakers.description')}</p>
        </div>
        <Button variant="outline" size="sm" onClick={() => void refresh()} disabled={loading || downloading}>
          {loading ? <Loader2 className="w-4 h-4 animate-spin" /> : <RefreshCw className="w-4 h-4" />}
          <span className="ml-2">{t('common.refresh')}</span>
        </Button>
      </div>

      {loading && !status ? (
        <div className="flex items-center gap-2 text-sm text-gray-500 p-4">
          <Loader2 className="w-4 h-4 animate-spin" />
          {t('common.loading')}
        </div>
      ) : status ? (
        <div className="p-4 border rounded-lg space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-2 min-w-0">
              {status.ready ? (
                <CheckCircle2 className="w-4 h-4 text-green-600 shrink-0" />
              ) : (
                <AlertTriangle className="w-4 h-4 text-amber-600 shrink-0" />
              )}
              <span className="text-sm font-medium text-gray-900">
                {status.ready ? t('speakers.ready') : t('speakers.missing')}
              </span>
            </div>

            {!status.ready && (
              <Button onClick={() => void handleDownload()} disabled={downloading || !status.has_room}>
                {downloading ? (
                  <Loader2 className="w-4 h-4 animate-spin" />
                ) : (
                  <Download className="w-4 h-4" />
                )}
                <span className="ml-2">
                  {downloading ? t('speakers.downloading') : t('speakers.download')}
                </span>
              </Button>
            )}

            {status.ready && (
              // Offered even when ready so a corrupted file can be replaced without
              // hunting for the folder.
              <Button variant="outline" size="sm" onClick={() => void handleDownload()} disabled={downloading}>
                {downloading ? <Loader2 className="w-4 h-4 animate-spin" /> : <Download className="w-4 h-4" />}
                <span className="ml-2">{t('speakers.redownload')}</span>
              </Button>
            )}
          </div>

          {!status.ready && (
            <p className="text-sm text-gray-600">
              {t('speakers.size', { size: requiredMb })}
            </p>
          )}

          {/* Not enough room is refused by the backend before anything is written, so
              saying so here is the difference between a clear message and a failure. */}
          {!status.has_room && (
            <p className="text-sm text-amber-700">
              {status.message}
            </p>
          )}

          <div className="flex items-center gap-2 text-xs text-gray-500 min-w-0">
            <span className="shrink-0">{t('speakers.folder')}:</span>
            <code className="truncate">{status.folder}</code>
            <button
              type="button"
              onClick={() => void handleOpenFolder()}
              className="shrink-0 inline-flex items-center gap-1 text-blue-600 hover:underline"
            >
              <FolderOpen className="w-3 h-3" />
              {t('speakers.openFolder')}
            </button>
          </div>

          <p className="text-xs text-gray-500">{t('speakers.staleNote')}</p>
        </div>
      ) : null}

      {/* Only meaningful once the models are present: the tunables change how a run
          behaves, and offering them before the feature can run at all is noise. */}
      {status?.ready && <SpeakerSeparationTuning />}
    </div>
  );
}
