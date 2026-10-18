'use client';

import { useState, useEffect } from 'react';
import { RecordingControls } from '@/components/RecordingControls';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { StatusOverlays } from '@/app/_components/StatusOverlays';
import Analytics from '@/lib/analytics';
import { SettingsModals } from './_components/SettingsModal';
import { TranscriptPanel } from './_components/TranscriptPanel';
import { useModalState } from '@/hooks/useModalState';
import { useRecordingStateSync } from '@/hooks/useRecordingStateSync';
import { useRecordingStart } from '@/hooks/useRecordingStart';
import { listen } from '@tauri-apps/api/event';
import { useRecordingStop } from '@/hooks/useRecordingStop';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { PostRecordingDialog } from '@/components/PostRecordingDialog';
import { indexedDBService } from '@/services/indexedDBService';
import { toast } from 'sonner';
import { useRouter } from 'next/navigation';
import { HomeDashboard } from '@/components/home/HomeDashboard';

export default function Home() {
  // Local page state (not moved to contexts)
  const [isRecording, setIsRecordingState] = useState(false);
  // Heard audio only: the three bars beside the stop button used to be random numbers
  // on a timer, which read as a working meter and told the user nothing. They are now
  // the loudest device's actual level, rolling left so movement is visible.
  const [barHeights, setBarHeights] = useState(['6%', '6%', '6%']);
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);

  // Use contexts for state management
  const { meetingTitle, transcripts } = useTranscripts();
  const { transcriptModelConfig, selectedDevices, captureMicrophone } = useConfig();
  const recordingState = useRecordingState();

  // Extract status from global state
  const { status, isStopping, isProcessing, isSaving } = recordingState;

  // Hooks
  const { hasMicrophone } = usePermissionCheck();
  const { setIsMeetingActive, isCollapsed: sidebarCollapsed, refetchMeetings } = useSidebar();
  const { modals, messages, showModal, hideModal } = useModalState(transcriptModelConfig);
  const { isRecordingDisabled, setIsRecordingDisabled } = useRecordingStateSync(isRecording, setIsRecordingState, setIsMeetingActive);
  const { handleRecordingStart } = useRecordingStart(isRecording, setIsRecordingState, showModal);

  // Get handleRecordingStop function and setIsStopping (state comes from global context)
  const {
    handleRecordingStop,
    setIsStopping,
    pendingPostRecording,
    handlePostRecordingChoice,
    postRecordingProgress,
  } = useRecordingStop(
    setIsRecordingState,
    setIsRecordingDisabled
  );

  // Recovery hook
  const {
    recoverableMeetings,
    isLoading: isLoadingRecovery,
    isRecovering,
    checkForRecoverableTranscripts,
    recoverMeeting,
    loadMeetingTranscripts,
    deleteRecoverableMeeting
  } = useTranscriptRecovery();

  const router = useRouter();

  useEffect(() => {
    // Track page view
    Analytics.trackPageView('home');
  }, []);

  // Startup recovery check
  useEffect(() => {
    const performStartupChecks = async () => {
      try {
        // Skip recovery check if currently recording or processing stop
        // This prevents the recovery dialog from showing when:
        if (recordingState.isRecording ||
          status === RecordingStatus.STOPPING ||
          status === RecordingStatus.PROCESSING_TRANSCRIPTS ||
          status === RecordingStatus.SAVING) {
          console.log('Skipping recovery check - recording in progress or processing');
          return;
        }

        // 1. Clean up old meetings (7+ days)
        try {
          await indexedDBService.deleteOldMeetings(7);
        } catch (error) {
          console.warn('⚠️ Failed to clean up old meetings:', error);
        }

        // 2. Clean up saved meetings (24+ hours after save)
        try {
          await indexedDBService.deleteSavedMeetings(24);
        } catch (error) {
          console.warn('⚠️ Failed to clean up saved meetings:', error);
        }

        // 3. Always check for recoverable meetings on startup
        // Don't skip based on sessionStorage - we need to check every time
        await checkForRecoverableTranscripts();
      } catch (error) {
        console.error('Failed to perform startup checks:', error);
      }
    };

    performStartupChecks();
  }, [checkForRecoverableTranscripts, recordingState.isRecording, status]);

  // Watch for recoverable meetings changes and show dialog once per session
  useEffect(() => {
    // Only show dialog if we have meetings and haven't shown it yet this session
    if (recoverableMeetings.length > 0) {
      const shownThisSession = sessionStorage.getItem('recovery_dialog_shown');
      if (!shownThisSession) {
        setShowRecoveryDialog(true);
        sessionStorage.setItem('recovery_dialog_shown', 'true');
      }
    }
  }, [recoverableMeetings]);

  // Handle recovery with toast notifications and navigation
  const handleRecovery = async (meetingId: string) => {
    try {
      const result = await recoverMeeting(meetingId);

      if (result.success) {
        toast.success('Meeting recovered successfully!', {
          description: result.audioRecoveryStatus?.status === 'success'
            ? 'Transcripts and audio recovered'
            : 'Transcripts recovered (no audio available)',
          action: result.meetingId ? {
            label: 'View Meeting',
            onClick: () => {
              router.push(`/meeting-details?id=${result.meetingId}`);
            }
          } : undefined,
          duration: 10000,
        });

        // Refresh sidebar to show the newly recovered meeting
        await refetchMeetings();

        // If no more recoverable meetings, clear session flag so dialog can show again
        if (recoverableMeetings.length === 0) {
          sessionStorage.removeItem('recovery_dialog_shown');
        }

        // Auto-navigate after a short delay
        if (result.meetingId) {
          setTimeout(() => {
            router.push(`/meeting-details?id=${result.meetingId}`);
          }, 2000);
        }
      }
    } catch (error) {
      toast.error('Failed to recover meeting', {
        description: error instanceof Error ? error.message : 'Unknown error occurred',
      });
      throw error;
    }
  };

  // Handle dialog close - clear session flag if no meetings left
  const handleDialogClose = () => {
    setShowRecoveryDialog(false);
    // If user closes dialog and there are no more meetings, clear the flag
    // This allows the dialog to show again next session if new meetings appear
    if (recoverableMeetings.length === 0) {
      sessionStorage.removeItem('recovery_dialog_shown');
    }
  };

  useEffect(() => {
    if (!recordingState.isRecording) {
      setBarHeights(['6%', '6%', '6%']);
      return;
    }

    let unlisten: (() => void) | undefined;
    let cancelled = false;

    listen<{ levels: Array<{ rms_level: number }> }>('audio-levels', event => {
      const levels = event.payload?.levels ?? [];
      if (levels.length === 0) return;

      // The loudest device wins: during a meeting the interesting question is whether
      // anything at all is being captured, not which device is marginally louder.
      const loudest = Math.max(...levels.map(level => level.rms_level ?? 0));

      // Square root, because perceived loudness is not linear in RMS: a linear bar
      // spends most of its range near the bottom and looks broken.
      const percent = Math.min(100, Math.round(Math.sqrt(Math.max(0, loudest)) * 140));

      setBarHeights(previous => [previous[1], previous[2], `${Math.max(6, percent)}%`]);
    })
      .then(stop => {
        // The listener may resolve after the effect was cleaned up.
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(error => console.error('Failed to subscribe to input levels:', error));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [recordingState.isRecording]);

  // Computed values using global status
  const isProcessingStop = status === RecordingStatus.PROCESSING_TRANSCRIPTS || isProcessing;

  // Idle with nothing on screen: show the dashboard instead of an empty transcript.
  // The recording controls stay mounted (hidden) so their listeners are unaffected.
  const showDashboard =
    !recordingState.isRecording &&
    !isStopping &&
    !isProcessingStop &&
    !isSaving &&
    status !== RecordingStatus.STOPPING &&
    status !== RecordingStatus.SAVING &&
    transcripts.length === 0;

  return (
    <div className="flex flex-col h-screen bg-background">
      {/* All Modals supported*/}
      <SettingsModals
        modals={modals}
        messages={messages}
        onClose={hideModal}
      />

      {/* Recovery Dialog */}
      <TranscriptRecovery
        isOpen={showRecoveryDialog}
        onClose={handleDialogClose}
        recoverableMeetings={recoverableMeetings}
        onRecover={handleRecovery}
        onDelete={deleteRecoverableMeeting}
        onLoadPreview={loadMeetingTranscripts}
      />

      {/* Ask what to do with a recording that has no live transcript. The audio
          is already saved by this point, so every choice keeps it. */}
      <PostRecordingDialog
        open={pendingPostRecording !== null}
        input={{
          // Reaching this dialog means live transcription was skipped.
          deferred: true,
          transcriptCount: 0,
          hasAudio: true,
        }}
        onChoose={handlePostRecordingChoice}
        onDismiss={() => void handlePostRecordingChoice('keep')}
        progress={postRecordingProgress}
      />
      <div className="flex flex-1 overflow-hidden">
        {showDashboard ? (
          <HomeDashboard
            hasMicrophone={hasMicrophone}
            isStarting={status === RecordingStatus.STARTING}
            onStart={() => window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'))}
          />
        ) : (
          <TranscriptPanel
            isProcessingStop={isProcessingStop}
            isStopping={isStopping}
            showModal={showModal}
          />
        )}

        {/* Recording controls - only show when permissions are granted or already recording and not showing status messages */}
        {(hasMicrophone || !captureMicrophone || isRecording) &&
          status !== RecordingStatus.PROCESSING_TRANSCRIPTS &&
          status !== RecordingStatus.SAVING && (
            <div className={`fixed bottom-8 left-0 right-0 z-10 ${showDashboard ? 'hidden' : ''}`}>
              <div
                className="flex justify-center transition-[margin] duration-300"
                style={{
                  marginLeft: sidebarCollapsed ? '4rem' : '16rem'
                }}
              >
                <div className="w-2/3 max-w-[750px] flex justify-center">
                  <div className="bg-white/95 backdrop-blur rounded-full border border-slate-200 shadow-[0_12px_32px_-12px_rgba(15,23,42,0.25)] flex items-center">
                    <RecordingControls
                      isRecording={recordingState.isRecording}
                      onRecordingStop={(callApi = true) => handleRecordingStop(callApi)}
                      onRecordingStart={handleRecordingStart}
                      onTranscriptReceived={() => { }} // Not actually used by RecordingControls
                      onStopInitiated={() => setIsStopping(true)}
                      barHeights={barHeights}
                      onTranscriptionError={(message) => {
                        showModal('errorAlert', message);
                      }}
                      isRecordingDisabled={isRecordingDisabled}
                      isParentProcessing={isProcessingStop}
                      selectedDevices={selectedDevices}
                      meetingName={meetingTitle}
                    />
                  </div>
                </div>
              </div>
            </div>
          )}

        {/* Status Overlays - Processing and Saving */}
        <StatusOverlays
          isProcessing={status === RecordingStatus.PROCESSING_TRANSCRIPTS && !recordingState.isRecording}
          isSaving={status === RecordingStatus.SAVING}
          sidebarCollapsed={sidebarCollapsed}
        />
      </div>
    </div>
  );
}
