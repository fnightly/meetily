"use client";
import { useState, useEffect, useMemo, useRef } from 'react';
import { motion } from 'framer-motion';
import { MeetingSummary, SummaryProcessResponse } from '@/types';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { MeetingDetailsSplitView, type MeetingDetailsTab } from '@/components/MeetingDetails/MeetingDetailsSplitView';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { ExportDialog } from '@/components/ExportDialog';
import { SyncDialog } from '@/components/SyncDialog';
import { MeetingHeader } from '@/components/MeetingDetails/MeetingHeader';
import { hasVisibleSummaryContent } from '@/lib/summary-content';
import { useI18n } from '@/contexts/I18nContext';
import { MeetingSidePane } from '@/components/workspace/MeetingSidePane';
import { summaryToMarkdown } from '@/lib/meeting-export';
import { syncActionItemsFromMarkdown } from '@/lib/action-items-sync';

/** Shape returned by the `api_get_meeting` command (complete, unpaginated). */
interface MeetingDetailsResponse {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  transcripts: Array<{
    id: string;
    text: string;
    timestamp: string;
    audio_start_time?: number;
    audio_end_time?: number;
    duration?: number;
  }>;
}

// Custom hooks
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';

export default function PageContent({
  meeting,
  summaryData,
  initialSummary,
  shouldAutoGenerate = false,
  onAutoGenerateComplete,
  onMeetingUpdated,
  onRefetchTranscripts,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  focusTime = null,
  highlightQuery = null,
}: {
  meeting: any;
  summaryData: MeetingSummary | null;
  initialSummary: SummaryProcessResponse | null;
  shouldAutoGenerate?: boolean;
  onAutoGenerateComplete?: () => void;
  onMeetingUpdated?: () => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
  /** Deep link from search / Q&A: second to scroll to and term to mark. */
  focusTime?: number | null;
  highlightQuery?: string | null;
}) {

  // State
  const [customPrompt, setCustomPrompt] = useState<string>('');
  const isRecording = false;
  const [activeTab, setActiveTab] = useState<MeetingDetailsTab>('transcript');
  const [meetingAudioPath, setMeetingAudioPath] = useState<string | null>(null);
  const actionItemsSyncedRef = useRef<string | null>(null);
  const previousSummaryStatusRef = useRef<string | null>(null);
  const templateRecommendedRef = useRef(false);

  // The meeting page knows the folder, not the encoder's file name; ask the
  // backend so old meetings whose audio lives outside the current recordings
  // folder can still be played inline.
  useEffect(() => {
    let cancelled = false;
    setMeetingAudioPath(null);
    if (!meeting.folder_path) return;

    invoke<string | null>('get_recording_audio_path', { folderPath: meeting.folder_path })
      .then((path) => {
        if (!cancelled) setMeetingAudioPath(path);
      })
      .catch((error) => {
        console.warn('Could not resolve the meeting audio file:', error);
        if (!cancelled) setMeetingAudioPath(null);
      });

    return () => {
      cancelled = true;
    };
  }, [meeting.folder_path]);

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);
  const autoSwitchedSummaryMeetingIdsRef = useRef(new Set<string>());
  const manuallySelectedTabMeetingIdsRef = useRef(new Set<string>());
  const autoGenerationStartedMeetingIdRef = useRef<string | null>(null);

  // Sidebar context
  const { serverAddress } = useSidebar();

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig, isModelConfigLoading } = useConfig();

  // Custom hooks
  const { t } = useI18n();
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const templates = useTemplates();

  // Callback to register the modal open function
  const handleRegisterModalOpen = (openFn: () => void) => {
    openModelSettingsRef.current = openFn;
  };

  // Callback to trigger modal open (called from error handler)
  const handleOpenModelSettings = () => {
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  // Save model config to backend database and sync via event
  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success('Model settings saved successfully');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Failed to save model settings');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    initialSummary,
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading,
    selectedTemplate: templates.selectedTemplate,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  /**
   * Export opens the multi-format dialog. The complete transcript is re-read
   * from the backend there (this view paginates transcripts), while the
   * summary on screen — which may hold unsaved edits — is passed in.
   */
  const [exportOpen, setExportOpen] = useState(false);

  /**
   * Sync is a separate, explicit action from export: Obsidian writes locally,
   * but Notion uploads the note, so it must never ride along with exporting.
   */
  const [syncOpen, setSyncOpen] = useState(false);

  const summaryMarkdown = useMemo(() => {
    const md = summaryToMarkdown(meetingData.aiSummary);
    return md.trim() ? md : null;
  }, [meetingData.aiSummary]);

  // Follow-ups: import once from an existing summary, and again (forced) after
  // a generation finishes so a regenerated summary's new items appear.
  useEffect(() => {
    const status = summaryGeneration.summaryStatus;
    const previous = previousSummaryStatusRef.current;
    previousSummaryStatusRef.current = status;
    if (!summaryMarkdown) return;
    const justGenerated = status === 'completed' && previous !== null && previous !== 'completed';
    if (justGenerated) {
      void syncActionItemsFromMarkdown(meeting.id, summaryMarkdown, true).catch(() => {});
      actionItemsSyncedRef.current = meeting.id;
    } else if (actionItemsSyncedRef.current !== meeting.id) {
      actionItemsSyncedRef.current = meeting.id;
      void syncActionItemsFromMarkdown(meeting.id, summaryMarkdown, false).catch(() => {});
    }
  }, [meeting.id, summaryMarkdown, summaryGeneration.summaryStatus]);

  // Suggest a template from the title and the opening of the transcript.
  useEffect(() => {
    if (templateRecommendedRef.current || templates.availableTemplates.length === 0) return;
    const sample = meetingData.transcripts.slice(0, 40).map((seg: { text: string }) => seg.text).join(' ');
    if (!sample && !(meetingData.meetingTitle || meeting.title)) return;
    templateRecommendedRef.current = true;
    templates.recommendFor(meetingData.meetingTitle || meeting.title, sample);
  }, [templates.availableTemplates.length, templates.recommendFor, meetingData.transcripts, meetingData.meetingTitle, meeting.title]);

  // Track page view
  useEffect(() => {
  }, []);

  useEffect(() => {
    if (
      (meetingData.aiSummary || summaryGeneration.summaryStatus === 'completed')
      && !autoSwitchedSummaryMeetingIdsRef.current.has(meeting.id)
      && !manuallySelectedTabMeetingIdsRef.current.has(meeting.id)
    ) {
      autoSwitchedSummaryMeetingIdsRef.current.add(meeting.id);
      setActiveTab('summary');
    }
  }, [meeting.id, meetingData.aiSummary, summaryGeneration.summaryStatus]);

  // Auto-generate only after the model configuration has settled.
  useEffect(() => {
    if (
      !shouldAutoGenerate
      || summaryGeneration.summaryStatus !== 'idle'
      || isModelConfigLoading
      || meetingData.transcripts.length === 0
      || autoGenerationStartedMeetingIdRef.current === meeting.id
    ) {
      return;
    }

    autoGenerationStartedMeetingIdRef.current = meeting.id;
    onAutoGenerateComplete?.();
    void summaryGeneration.handleGenerateSummary('');
  }, [
    shouldAutoGenerate,
    meeting.id,
    meetingData.transcripts.length,
    isModelConfigLoading,
    modelConfig.provider,
    modelConfig.model,
    summaryGeneration.handleGenerateSummary,
    summaryGeneration.summaryStatus,
    onAutoGenerateComplete,
  ]);

  // Only trusted once every segment is loaded; a partial page would understate it.
  const meetingDurationSeconds = (() => {
    if (hasMore) return null;
    const list: any[] = segments ?? meetingData.transcripts ?? [];
    let max = 0;
    for (const seg of list) {
      const end = Number(seg?.endTime ?? seg?.audio_end_time ?? seg?.timestamp ?? seg?.audio_start_time ?? 0);
      if (Number.isFinite(end) && end > max) max = end;
    }
    return max > 0 ? max : null;
  })();

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col h-screen min-w-0 bg-background"
    >
      <MeetingHeader
        meetingId={meeting.id}
        title={meetingData.meetingTitle || meeting.title || t('live.untitled')}
        createdAt={meeting.created_at}
        segmentCount={totalCount ?? meetingData.transcripts.length}
        durationSeconds={meetingDurationSeconds}
        hasSummary={hasVisibleSummaryContent(meetingData.aiSummary)}
        onTitleSaved={meetingData.updateMeetingTitle}
        onExport={() => setExportOpen(true)}
        onSync={() => setSyncOpen(true)}
      />
      <div className="flex flex-1 min-w-0 overflow-hidden">
        <MeetingDetailsSplitView
          activeTab={activeTab}
          onTabChange={(tab) => {
            manuallySelectedTabMeetingIdsRef.current.add(meeting.id);
            setActiveTab(tab);
          }}
          transcript={
            <TranscriptPanel
              transcripts={meetingData.transcripts}
              customPrompt={customPrompt}
              onPromptChange={setCustomPrompt}
              onCopyTranscript={copyOperations.handleCopyTranscript}
              onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
              isRecording={isRecording}
              disableAutoScroll={true}
              usePagination={true}
              segments={segments}
              hasMore={hasMore}
              isLoadingMore={isLoadingMore}
              totalCount={totalCount}
              loadedCount={loadedCount}
              onLoadMore={onLoadMore}
              meetingId={meeting.id}
              meetingFolderPath={meeting.folder_path}
              meetingTitle={meetingData.meetingTitle || meeting.title}
              audioPath={meetingAudioPath}
              onRefetchTranscripts={onRefetchTranscripts}
              onExportMarkdown={() => setExportOpen(true)}
              isExporting={exportOpen}
              focusTime={focusTime}
              highlightQuery={highlightQuery}
            />
          }
          summary={
            <MeetingSidePane
              meetingId={meeting.id}
              summaryMarkdown={summaryMarkdown}
              summary={
            <SummaryPanel
              meeting={meeting}
              meetingTitle={meetingData.meetingTitle}
              summaryRef={meetingData.blockNoteSummaryRef}
              isSaving={meetingData.isSaving}
              isSummaryDirty={meetingData.isSummaryDirty}
              onSaveAll={meetingData.saveAllChanges}
              onCopySummary={copyOperations.handleCopySummary}
              aiSummary={meetingData.aiSummary}
              summaryStatus={summaryGeneration.summaryStatus}
              transcripts={meetingData.transcripts}
              modelConfig={modelConfig}
              setModelConfig={setModelConfig}
              onSaveModelConfig={handleSaveModelConfig}
              onGenerateSummary={summaryGeneration.handleGenerateSummary}
              onStopGeneration={summaryGeneration.handleStopGeneration}
              customPrompt={customPrompt}
              onSaveSummary={meetingData.handleSaveSummary}
              onSummaryChange={meetingData.handleSummaryChange}
              onDirtyChange={meetingData.setIsSummaryDirty}
              summaryError={summaryGeneration.summaryError}
              onRegenerateSummary={summaryGeneration.handleRegenerateSummary}
              getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
              availableTemplates={templates.availableTemplates}
              selectedTemplate={templates.selectedTemplate}
              onTemplateSelect={templates.handleTemplateSelection}
              templateActions={{
                loadTemplate: templates.loadTemplate,
                saveTemplate: templates.saveTemplate,
                duplicateTemplate: templates.duplicateTemplate,
                deleteTemplate: templates.deleteTemplate,
                getCustomTemplatesDir: templates.getCustomTemplatesDir,
              }}
              isModelConfigLoading={isModelConfigLoading}
              onOpenModelSettings={handleRegisterModalOpen}
              recommendedTemplate={templates.recommendedTemplate}
            />
              }
            />
          }
        />
      </div>
      <SyncDialog
        open={syncOpen}
        onOpenChange={setSyncOpen}
        meetingId={meeting.id}
        title={meetingData.meetingTitle || meeting.title}
        summaryOverride={meetingData.aiSummary}
      />
      <ExportDialog
        open={exportOpen}
        onOpenChange={setExportOpen}
        meetingIds={[meeting.id]}
        title={meetingData.meetingTitle || meeting.title}
        summaryOverride={meetingData.aiSummary}
      />
    </motion.div>
  );
}
