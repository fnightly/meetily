"use client";

import { useState, useCallback, useEffect } from 'react';
import { Button } from '@/components/ui/button';
import { Copy, FolderOpen, RefreshCw, Users } from 'lucide-react';
import { RetranscribeDialog } from './RetranscribeDialog';
import { useConfig } from '@/contexts/ConfigContext';
import { useI18n } from '@/contexts/I18nContext';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';


interface TranscriptButtonGroupProps {
  transcriptCount: number;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
  /** Export the meeting as Markdown (transcript + summary). */
  onExportMarkdown?: () => void | Promise<void>;
  isExporting?: boolean;
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
  onExportMarkdown,
  isExporting = false,
}: TranscriptButtonGroupProps) {
  const { betaFeatures } = useConfig();
  const { t } = useI18n();
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);
  const [identifying, setIdentifying] = useState(false);
  const [progress, setProgress] = useState<{ phase: string; percent: number } | null>(null);

  /**
   * Follow a separation run's progress.
   *
   * A run over a long meeting takes minutes, and the invoke does not return until it ends,
   * so without this the button would spin with nothing to show for the wait.
   */
  useEffect(() => {
    if (!identifying || !meetingId) return;

    let unlisten: (() => void) | undefined;
    let cancelled = false;

    void listen<{ meeting_id: string; phase: string; percent: number }>(
      'diarization-progress',
      event => {
        // Other windows may be separating a different meeting; ignore their events.
        if (event.payload.meeting_id !== meetingId) return;
        setProgress({ phase: event.payload.phase, percent: event.payload.percent });
      }
    ).then(stop => {
      // The listener may arrive after the run finished, in which case it must not leak.
      if (cancelled) stop();
      else unlisten = stop;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [identifying, meetingId]);

  /** Caption for the phase a run is in. */
  const phaseCaption = (phase: string): string => {
    switch (phase) {
      case 'segmenting':
        return t('speakers.phaseSegmenting');
      case 'embedding':
        return t('speakers.phaseEmbedding');
      case 'clustering':
        return t('speakers.phaseClustering');
      default:
        return t('speakers.running');
    }
  };

  /**
   * Run speaker separation over the saved recording.
   *
   * Explicit rather than automatic: it takes minutes on a long meeting and the labels it
   * writes change how the transcript reads, so the user should ask for it.
   */
  const handleIdentifySpeakers = useCallback(async () => {
    if (!meetingId || !meetingFolderPath) return;

    setIdentifying(true);
    setProgress(null);
    try {

      const result = await invoke<{
        speaker_count: number;
        labelled_segments: number;
      }>('diarize_meeting', { meetingId, meetingFolderPath });

      if (result.speaker_count === 0) {
        toast.info(t('speakers.runNone'));
      } else {
        toast.success(t('speakers.runDone', { count: result.speaker_count }));
      }

      // Reload so the new labels appear.
      if (onRefetchTranscripts) await onRefetchTranscripts();
    } catch (error) {
      console.error('Speaker separation failed:', error);
      toast.error(t('speakers.runFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setIdentifying(false);
    }
    // `t` is stable for a given locale, so including it is correct: a language change
    // should rebuild the handler's messages.
  }, [meetingId, meetingFolderPath, onRefetchTranscripts, t]);

  const handleRetranscribeComplete = useCallback(async () => {
    // Refetch transcripts to show the updated data
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  return (
    <div className="flex items-center justify-between w-full gap-2 min-w-0">
      <div className="flex items-baseline gap-2 min-w-0">
        <h2 className="text-sm font-semibold text-slate-900 whitespace-nowrap">{t('meeting.transcriptPane')}</h2>
        <span className="text-xs text-slate-400 tabular-nums">{transcriptCount}</span>
      </div>
      <div className="flex items-center gap-0.5 flex-shrink-0">
        <Button
          variant="ghost"
          size="sm"
          className="px-2 @[34rem]:px-3"
          onClick={() => {
            onCopyTranscript();
          }}
          disabled={transcriptCount === 0}
          title={transcriptCount === 0 ? t('transcriptActions.noTranscript') : t('transcriptActions.copy')}
        >
          <Copy />
          <span className="hidden @[34rem]:inline">{t('transcriptActions.copyShort')}</span>
        </Button>

        <Button
          size="sm"
          variant="ghost"
          className="px-2 @[34rem]:px-3"
          onClick={() => {
            onOpenMeetingFolder();
          }}
          title={t('transcriptActions.openFolder')}
        >
          <FolderOpen className="@[34rem]:mr-1" size={16} />
          <span className="hidden @[34rem]:inline">{t('transcriptActions.folderShort')}</span>
        </Button>

        {meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="ghost"
            className="px-2 @[34rem]:px-3"
            onClick={() => void handleIdentifySpeakers()}
            disabled={identifying || transcriptCount === 0}
            // Say why it is unavailable. Separation labels transcript segments, so with
            // no transcript there is nothing to label — which is not obvious from a
            // merely greyed-out button.
            title={transcriptCount === 0 ? t('speakers.runNeedsTranscript') : t('speakers.runHint')}
          >
            {identifying ? (
              <RefreshCw className="animate-spin @[34rem]:mr-1" size={16} />
            ) : (
              <Users className="@[34rem]:mr-1" size={16} />
            )}
            <span className="hidden @[34rem]:inline">
              {identifying
                ? progress
                  ? `${phaseCaption(progress.phase)} ${progress.percent}%`
                  : t('speakers.running')
                : t('speakers.run')}
            </span>
          </Button>
        )}

        {meetingId && meetingFolderPath && (betaFeatures.importAndRetranscribe || transcriptCount === 0) && (
          <Button
            size="sm"
            variant={transcriptCount === 0 ? 'default' : 'ghost'}
            className={transcriptCount === 0
              ? 'px-2 @[34rem]:px-3'
              : 'px-2 @[34rem]:px-3 text-blue-700 hover:bg-blue-50'}
            onClick={() => {
              setShowRetranscribeDialog(true);
            }}
            title={transcriptCount === 0
              ? t('transcriptActions.transcribeRecordingHint')
              : t('transcriptActions.retranscribeHint')}
          >
            <RefreshCw className="@[34rem]:mr-1" size={16} />
            <span className="hidden @[34rem]:inline">
              {transcriptCount === 0
                ? t('transcriptActions.transcribeRecording')
                : t('transcriptActions.retranscribe')}
            </span>
          </Button>
        )}
      </div>

      {meetingId && meetingFolderPath && (betaFeatures.importAndRetranscribe || transcriptCount === 0) && (
        <RetranscribeDialog
          open={showRetranscribeDialog}
          onOpenChange={setShowRetranscribeDialog}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          emptyTranscript={transcriptCount === 0}
          onComplete={handleRetranscribeComplete}
        />
      )}
    </div>
  );
}
