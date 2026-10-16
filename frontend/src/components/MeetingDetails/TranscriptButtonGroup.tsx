"use client";

import { useState, useCallback, useEffect } from 'react';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy, FolderOpen, RefreshCw, Download, Users } from 'lucide-react';
import Analytics from '@/lib/analytics';
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
      Analytics.trackButtonClick('identify_speakers', 'meeting_details');

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
    <div className="flex items-center justify-center w-full gap-2">
      <ButtonGroup>
        <Button
          variant="outline"
          size="sm"
          className="px-2 @[22rem]:px-3"
          onClick={() => {
            Analytics.trackButtonClick('copy_transcript', 'meeting_details');
            onCopyTranscript();
          }}
          disabled={transcriptCount === 0}
          title={transcriptCount === 0 ? 'No transcript available' : 'Copy Transcript'}
        >
          <Copy />
          <span className="hidden @[22rem]:inline">Copy</span>
        </Button>

        <Button
          variant="outline"
          size="sm"
          className="px-2 @[22rem]:px-3"
          onClick={() => {
            Analytics.trackButtonClick('export_markdown', 'meeting_details');
            onExportMarkdown?.();
          }}
          disabled={isExporting}
          title="Export this meeting as Markdown (summary + transcript)"
        >
          <Download />
          <span className="hidden @[22rem]:inline">{isExporting ? 'Exporting…' : 'Export'}</span>
        </Button>

        <Button
          size="sm"
          variant="outline"
          className="px-2 @[22rem]:px-4"
          onClick={() => {
            Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
            onOpenMeetingFolder();
          }}
          title="Open Recording Folder"
        >
          <FolderOpen className="@[22rem]:mr-2" size={18} />
          <span className="hidden @[22rem]:inline">Recording</span>
        </Button>

        {meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="outline"
            className="px-2 @[22rem]:px-4"
            onClick={() => void handleIdentifySpeakers()}
            disabled={identifying || transcriptCount === 0}
            // Say why it is unavailable. Separation labels transcript segments, so with
            // no transcript there is nothing to label — which is not obvious from a
            // merely greyed-out button.
            title={transcriptCount === 0 ? t('speakers.runNeedsTranscript') : t('speakers.runHint')}
          >
            {identifying ? (
              <RefreshCw className="animate-spin @[22rem]:mr-2" size={18} />
            ) : (
              <Users className="@[22rem]:mr-2" size={18} />
            )}
            <span className="hidden @[22rem]:inline">
              {identifying
                ? progress
                  ? `${phaseCaption(progress.phase)} ${progress.percent}%`
                  : t('speakers.running')
                : t('speakers.run')}
            </span>
          </Button>
        )}

        {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="outline"
            className="bg-gradient-to-r from-blue-50 to-purple-50 hover:from-blue-100 hover:to-purple-100 border-blue-200 px-2 @[22rem]:px-4"
            onClick={() => {
              Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
              setShowRetranscribeDialog(true);
            }}
            title="Retranscribe to enhance your recorded audio"
          >
            <RefreshCw className="@[22rem]:mr-2" size={18} />
            <span className="hidden @[22rem]:inline">Enhance</span>
          </Button>
        )}
      </ButtonGroup>

      {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
        <RetranscribeDialog
          open={showRetranscribeDialog}
          onOpenChange={setShowRetranscribeDialog}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onComplete={handleRetranscribeComplete}
        />
      )}
    </div>
  );
}
