import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { PermissionWarning } from '@/components/PermissionWarning';
import { Button } from '@/components/ui/button';
import { Copy, GlobeIcon } from 'lucide-react';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { ModalType } from '@/hooks/useModalState';
import { useIsLinux } from '@/hooks/usePlatform';
import { useMemo } from 'react';
import { useI18n } from '@/contexts/I18nContext';
import { RecordingStatusBar } from '@/components/RecordingStatusBar';

/**
 * TranscriptPanel Component
 *
 * Displays transcript content with controls for copying and language settings.
 * Uses TranscriptContext, ConfigContext, and RecordingStateContext internally.
 */

interface TranscriptPanelProps {
  // indicates stop-processing state for transcripts; derived from backend statuses.
  isProcessingStop: boolean;
  isStopping: boolean;
  showModal: (name: ModalType, message?: string) => void;
}

export function TranscriptPanel({
  isProcessingStop,
  isStopping,
  showModal
}: TranscriptPanelProps) {
  // Contexts
  const { transcripts, transcriptContainerRef, copyTranscript, meetingTitle } = useTranscripts();
  const { t } = useI18n();
  const { transcriptModelConfig } = useConfig();
  const { isRecording, isPaused } = useRecordingState();
  const { checkPermissions, isChecking, hasSystemAudio, hasMicrophone } = usePermissionCheck();
  const isLinux = useIsLinux();

  // Convert transcripts to segments for virtualized view
  const segments = useMemo(() =>
    transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
      // Present for consistency with the meeting view. This panel shows live audio, which
      // is never separated, so the value is normally undefined — but a mapping that
      // silently drops the field is a trap for whoever next reuses this panel.
      speaker: t.speaker,
    })),
    [transcripts]
  );

  return (
    <div ref={transcriptContainerRef} className="w-full bg-white flex flex-col overflow-y-auto custom-scrollbar">
      {/* Sticky header: what is being recorded, its state, and the two live tools */}
      <div className="sticky top-0 z-10 border-b border-slate-100 bg-white/90 backdrop-blur">
        <div className="mx-auto flex w-full max-w-[860px] items-center gap-3 px-6 py-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              {isRecording ? (
                <RecordingStatusBar isPaused={isPaused} />
              ) : (isProcessingStop || isStopping) ? (
                <span className="chip bg-blue-50 text-blue-700">{t('live.finishing')}</span>
              ) : (
                <span className="chip bg-slate-100 text-slate-600">{t('live.lastSession')}</span>
              )}
              <span className="text-xs text-slate-400 tabular-nums">{t('live.segments', { count: transcripts.length })}</span>
            </div>
            <h1 className="mt-1 truncate text-base font-semibold text-slate-900">{meetingTitle && meetingTitle !== '+ New Call' ? meetingTitle : t('live.untitled')}</h1>
          </div>
          <div className="flex items-center gap-1.5">
            {transcriptModelConfig.provider === "localWhisper" && (
              <Button variant="ghost" size="sm" onClick={() => showModal('languageSettings')} title={t('live.language')}>
                <GlobeIcon />
                <span className="hidden md:inline">{t('live.language')}</span>
              </Button>
            )}
            {transcripts?.length > 0 && (
              <Button variant="outline" size="sm" onClick={copyTranscript} title={t('transcriptActions.copy')}>
                <Copy />
                <span className="hidden md:inline">{t('transcriptActions.copyShort')}</span>
              </Button>
            )}
          </div>
        </div>
      </div>

      {/* Permission Warning - Not needed on Linux */}
      {!isRecording && !isChecking && !isLinux && (
        <div className="flex justify-center px-4 pt-4">
          <PermissionWarning
            hasMicrophone={hasMicrophone}
            hasSystemAudio={hasSystemAudio}
            onRecheck={checkPermissions}
            isRechecking={isChecking}
          />
        </div>
      )}

      {/* Transcript content */}
      <div className="pb-32">
        <div className="flex justify-center">
          <div className="w-full max-w-[860px] px-6">
            <VirtualizedTranscriptView
              segments={segments}
              isRecording={isRecording}
              isPaused={isPaused}
              isProcessing={isProcessingStop}
              isStopping={isStopping}
              enableStreaming={isRecording}
              showConfidence={true}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
