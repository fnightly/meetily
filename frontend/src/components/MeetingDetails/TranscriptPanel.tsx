"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { TranscriptView } from '@/components/TranscriptView';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useCallback, useMemo, useState } from 'react';
import { SpeakerNamesProvider } from '@/lib/speaker-names';
import { SpeakerRenameDialog } from '@/components/workspace/SpeakerRenameDialog';
import { Sparkles } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { RecordingAudioPreview } from '@/components/RecordingAudioPreview';

interface TranscriptPanelProps {
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  /** Names the recording in background transcription messages. */
  meetingTitle?: string;
  /** Absolute audio path for the saved recording, when it still exists. */
  audioPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;

  // Markdown export
  onExportMarkdown?: () => void | Promise<void>;
  isExporting?: boolean;

  /** Deep-link target (seconds) and search term to mark. */
  focusTime?: number | null;
  highlightQuery?: string | null;
}

export function TranscriptPanel({
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  meetingTitle,
  audioPath = null,
  onRefetchTranscripts,
  onExportMarkdown,
  isExporting = false,
  focusTime = null,
  highlightQuery = null,
}: TranscriptPanelProps) {
  const { t } = useI18n();
  const [renaming, setRenaming] = useState<string | null>(null);
  const requestRename = useCallback((label: string) => setRenaming(label), []);
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
      // Carried through so the list can group consecutive turns by speaker.
      speaker: t.speaker,
    }));
  }, [transcripts, usePagination, segments]);

  const renameSample = useMemo(() => {
    if (!renaming) return null;
    const lines = convertedSegments.filter((s) => s.speaker === renaming && s.text.trim().length > 6);
    return lines.length ? lines.reduce((a, b) => (b.text.length > a.text.length && b.text.length < 140 ? b : a)).text : null;
  }, [renaming, convertedSegments]);

  return (
    <SpeakerNamesProvider meetingId={meetingId ?? null} onRequestRename={meetingId ? requestRename : undefined}>
    <div className="flex h-full min-w-0 w-full bg-white flex-col relative @container">
      {/* Title area */}
      <div className="px-4 py-2.5 border-b border-slate-200">
        <TranscriptButtonGroup
          transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
          onCopyTranscript={onCopyTranscript}
          onOpenMeetingFolder={onOpenMeetingFolder}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          meetingTitle={meetingTitle}
          onRefetchTranscripts={onRefetchTranscripts}
          onExportMarkdown={onExportMarkdown}
          isExporting={isExporting}
        />
      </div>

      {audioPath && (
        <div className="px-4 pt-2.5">
          <RecordingAudioPreview audioPath={audioPath} />
        </div>
      )}

      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          focusTime={focusTime}
          highlightQuery={highlightQuery}
          emptyTitle={audioPath ? t('meeting.emptyRecordedTitle') : undefined}
          emptyHint={audioPath ? t('meeting.emptyRecordedHint') : undefined}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && convertedSegments.length > 0 && (
        <div className="p-3 border-t border-slate-200 bg-slate-50/60">
          <label className="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-slate-600">
            <Sparkles className="h-3.5 w-3.5 text-violet-500" />{t('meeting.contextLabel')}
          </label>
          <textarea
            placeholder={t('meeting.contextPlaceholder')}
            className="w-full px-3 py-2 border border-slate-200 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-400 bg-white min-h-[64px] resize-y"
            value={customPrompt}
            onChange={(e) => onPromptChange(e.target.value)}
          />
        </div>
      )}
      <SpeakerRenameDialog label={renaming} sample={renameSample} onClose={() => setRenaming(null)} />
    </div>
    </SpeakerNamesProvider>
  );
}
