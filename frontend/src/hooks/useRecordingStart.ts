import { useState, useEffect, useCallback, useRef } from 'react';
import { tr } from '@/lib/i18n';
import { invoke } from '@tauri-apps/api/core';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import { isSameWindow, type RecordableWindow } from '@/lib/window-picker';
import { showRecordingNotification } from '@/lib/recordingNotification';
import {
  getProviderCommands,
  hasDownloadingModel,
  type ModelWithStatus,
} from '@/lib/transcription-model-readiness';
import { toast } from 'sonner';

const TRANSCRIPTION_RUNTIME_START_ERROR_CODE = 'TRANSCRIPTION_RUNTIME_INITIALIZATION_FAILED';
const TRANSCRIPTION_RUNTIME_USER_MESSAGE = 'Speech recognition could not initialize. Restart 会见. If the problem continues, repair or reinstall the app.';

const isTranscriptionRuntimeStartError = (error: unknown) =>
  String(error) === TRANSCRIPTION_RUNTIME_START_ERROR_CODE;

interface UseRecordingStartReturn {
  handleRecordingStart: () => Promise<void>;
  isAutoStarting: boolean;
}

interface TranscriptConfig {
  provider?: string;
}

/**
 * Custom hook for managing recording start lifecycle.
 * Handles both manual start (button click) and auto-start (from sidebar navigation).
 *
 * Features:
 * - Meeting title generation (format: Meeting DD_MM_YY_HH_MM_SS)
 * - Transcript clearing on start
 * - Recording notification display
 * - Auto-start from sidebar via sessionStorage flag
 */
export function useRecordingStart(
  isRecording: boolean,
  setIsRecording: (value: boolean) => void,
  showModal?: (name: 'modelSelector', message?: string) => void
): UseRecordingStartReturn {
  const [isAutoStarting, setIsAutoStarting] = useState(false);

  // Synchronous latch: a rapid double-click re-enters handleRecordingStart
  // before any state update lands, so an async/state guard can't stop it.
  const isStartingRef = useRef(false);

  const { clearTranscripts, setMeetingTitle } = useTranscripts();
  const { setIsMeetingActive } = useSidebar();
  const { selectedDevices, selectedWindow } = useConfig();
  const { setStatus } = useRecordingState();

  // Generate meeting title with timestamp
  const generateMeetingTitle = useCallback(() => {
    const now = new Date();
    const day = String(now.getDate()).padStart(2, '0');
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const year = String(now.getFullYear()).slice(-2);
    const hours = String(now.getHours()).padStart(2, '0');
    const minutes = String(now.getMinutes()).padStart(2, '0');
    const seconds = String(now.getSeconds()).padStart(2, '0');
    return `Meeting ${day}_${month}_${year}_${hours}_${minutes}_${seconds}`;
  }, []);

  // A window can close between being picked and the recording starting. Recording
  // its dead process id would capture silence on the system side without telling
  // the user, so the selection is re-checked against a fresh list first. A failed
  // list is not treated as stale: the backend already degrades to microphone-only.
  const selectedWindowIsStillOpen = useCallback(async (): Promise<boolean> => {
    if (!selectedWindow) return true;
    try {
      const windows = await invoke<RecordableWindow[]>('list_recordable_windows');
      if (windows.some((window) => isSameWindow(window, selectedWindow))) return true;
    } catch (error) {
      console.error('Failed to verify the selected window:', error);
      return true;
    }
    // The selection is kept rather than cleared: silently widening the next start
    // to whole-system audio is worse than asking the user to pick again.
    toast.error(tr('windowPicker.staleTitle'), {
      description: tr('windowPicker.staleHint'),
      duration: 6000,
      action: {
        label: tr('windowPicker.repick'),
        onClick: () => window.dispatchEvent(new CustomEvent('open-window-picker')),
      },
    });
    return false;
  }, [selectedWindow]);

  const getTranscriptionProvider = useCallback(async (): Promise<string> => {
    try {
      const config = await invoke<TranscriptConfig | null>('api_get_transcript_config');
      return config?.provider || 'parakeet';
    } catch (error) {
      console.error('Failed to load transcription provider:', error);
      return 'parakeet';
    }
  }, []);

  // Check the selected local transcription provider, not a hardcoded engine.
  const checkTranscriptionModelReady = useCallback(async (): Promise<boolean> => {
    try {
      const provider = await getTranscriptionProvider();
      const commands = getProviderCommands(provider);

      if (commands) {
        await invoke(commands.initialize);
        return await invoke<boolean>(commands.hasAvailableModels);
      }

      // Cloud providers have nothing to download: they are ready once the
      // backend marks them selectable and an API key is stored.
      const catalog = await invoke<Array<{ id: string; kind: string; selectable: boolean; has_api_key: boolean }>>(
        'api_list_speech_providers',
      );
      const cloud = catalog?.find((p) => p.id === provider && p.kind === 'cloud');
      if (cloud) {
        return cloud.selectable && cloud.has_api_key;
      }

      console.error(`Unsupported transcription provider: ${provider}`);
      return false;
    } catch (error) {
      console.error('Failed to check transcription model status:', error);
      return false;
    }
  }, [getTranscriptionProvider]);

  // Check download status for the selected local transcription provider.
  const checkIfModelDownloading = useCallback(async (): Promise<boolean> => {
    try {
      const provider = await getTranscriptionProvider();
      const commands = getProviderCommands(provider);
      if (!commands) return false;

      const models = await invoke<ModelWithStatus[]>(commands.getAvailableModels);
      return hasDownloadingModel(models);
    } catch (error) {
      console.error('Failed to check model download status:', error);
      return false; // Default to not downloading (will show error + modal)
    }
  }, [getTranscriptionProvider]);

  // Deferred transcription runs speech recognition after the recording, so a
  // missing model must not block starting it. Reading the preference here keeps
  // the start button working before any model has ever been downloaded.
  const transcriptionIsDeferred = useCallback(async (): Promise<boolean> => {
    try {
      const preferences = await invoke<{ defer_transcription?: boolean }>('get_recording_preferences');
      return preferences?.defer_transcription === true;
    } catch (error) {
      console.error('Failed to read whether transcription is deferred:', error);
      return false;
    }
  }, []);

  // The Rust recording command validates the same provider again before capture.
  const checkModelReady = useCallback(async (): Promise<boolean> => {
    if (await transcriptionIsDeferred()) return true;
    return checkTranscriptionModelReady();
  }, [transcriptionIsDeferred, checkTranscriptionModelReady]);

  // Handle manual recording start (from button click)
  const handleRecordingStart = useCallback(async () => {
    if (isStartingRef.current) {
      console.log('handleRecordingStart ignored - start already in progress');
      return;
    }
    isStartingRef.current = true;
    try {
      console.log('handleRecordingStart called - checking selected transcription model status');

      // Check the selected transcription model before starting.
      const modelReady = await checkModelReady();
      if (!modelReady) {
        const isDownloading = await checkIfModelDownloading();
        if (isDownloading) {
          toast.info(tr('toast.modelDownloading'), {
            description: tr('toast.modelDownloadingHint'),
            duration: 5000,
          });
        } else {
          toast.error(tr('toast.modelNotReady'), {
            description: tr('toast.modelNotReadyHint'),
            duration: 5000,
          });
          showModal?.('modelSelector', 'Transcription model setup required');
        }
        setStatus(RecordingStatus.IDLE);
        return;
      }

      // Give a closed window a chance to be re-picked instead of recording silence.
      if (!(await selectedWindowIsStillOpen())) {
        setStatus(RecordingStatus.IDLE);
        return;
      }

      console.log('Selected transcription model ready - setting up meeting title and state');

      const randomTitle = generateMeetingTitle();
      setMeetingTitle(randomTitle);

      // Set STARTING status before initiating backend recording
      setStatus(RecordingStatus.STARTING, 'Initializing recording...');

      // Start the actual backend recording
      console.log('Starting backend recording with meeting:', randomTitle);
      await recordingService.startRecordingWithDevices(
        selectedDevices?.micDevice || null,
        selectedDevices?.systemDevice || null,
        randomTitle,
        selectedWindow?.process_id ?? null
      );
      console.log('Backend recording started successfully');

      // Update state after successful backend start
      // Note: RECORDING status will be set by RecordingStateContext event listener
      console.log('Setting isRecordingState to true');
      setIsRecording(true); // This will also update the sidebar via the useEffect
      clearTranscripts(); // Clear previous transcripts when starting new recording
      setIsMeetingActive(true);

      // Show recording notification if enabled
      await showRecordingNotification();
    } catch (error) {
      console.error('Failed to start recording:', error);
      const errorMsg = error instanceof Error ? error.message : String(error);

      // A racing second start that lost to a live recording must not clobber
      // the running recording's state. The winning start is live, so reflect
      // RECORDING here — leaving STARTING latched would keep the Stop button
      // disabled forever, since it's gated on isStartingRecording.
      if (errorMsg.includes('already in progress')) {
        console.warn('Start rejected because recording is already active - leaving live recording state untouched');
        setStatus(RecordingStatus.RECORDING);
        return;
      }

      const isRuntimeError = isTranscriptionRuntimeStartError(error);
      if (errorMsg.includes('Recording start timed out')) {
        toast.error(tr('toast.startTimeout'));
      }

      setStatus(RecordingStatus.ERROR, isRuntimeError
        ? TRANSCRIPTION_RUNTIME_USER_MESSAGE
        : errorMsg);
      setIsRecording(false); // Reset state on error
      if (isRuntimeError) return;
      // Re-throw so RecordingControls can handle device-specific errors
      throw error;
    } finally {
      isStartingRef.current = false;
    }
  }, [generateMeetingTitle, setMeetingTitle, setIsRecording, clearTranscripts, setIsMeetingActive, checkModelReady, checkIfModelDownloading, selectedDevices, selectedWindowIsStillOpen, showModal, setStatus]);

  // Check for autoStartRecording flag and start recording automatically
  useEffect(() => {
    const checkAutoStartRecording = async () => {
      if (typeof window !== 'undefined') {
        const shouldAutoStart = sessionStorage.getItem('autoStartRecording');
        if (shouldAutoStart === 'true' && !isRecording && !isAutoStarting) {
          console.log('Auto-starting recording from navigation...');
          setIsAutoStarting(true);
          sessionStorage.removeItem('autoStartRecording'); // Clear the flag

          // Check the selected transcription model before starting.
          const modelReady = await checkModelReady();
          if (!modelReady) {
            const isDownloading = await checkIfModelDownloading();
            if (isDownloading) {
              toast.info(tr('toast.modelDownloading'), {
                description: tr('toast.modelDownloadingHint'),
                duration: 5000,
              });
            } else {
              toast.error(tr('toast.modelNotReady'), {
                description: tr('toast.modelNotReadyHint'),
                duration: 5000,
              });
              showModal?.('modelSelector', 'Transcription model setup required');
            }
            setStatus(RecordingStatus.IDLE);
            setIsAutoStarting(false);
            return;
          }

          // Start the actual backend recording
          try {
            if (!(await selectedWindowIsStillOpen())) {
              setStatus(RecordingStatus.IDLE);
              return;
            }

            // Generate meeting title
            const generatedMeetingTitle = generateMeetingTitle();

            // Set STARTING status before initiating backend recording
            setStatus(RecordingStatus.STARTING, 'Initializing recording...');

            console.log('Auto-starting backend recording with meeting:', generatedMeetingTitle);
            const result = await recordingService.startRecordingWithDevices(
              selectedDevices?.micDevice || null,
              selectedDevices?.systemDevice || null,
              generatedMeetingTitle,
              selectedWindow?.process_id ?? null
            );
            console.log('Auto-start backend recording result:', result);

            // Update UI state after successful backend start
            // Note: RECORDING status will be set by RecordingStateContext event listener
            setMeetingTitle(generatedMeetingTitle);
            setIsRecording(true);
            clearTranscripts();
            setIsMeetingActive(true);

            // Show recording notification if enabled
            await showRecordingNotification();
          } catch (error) {
            console.error('Failed to auto-start recording:', error);
            const errorMsg = error instanceof Error ? error.message : String(error);
            if (errorMsg.includes('already in progress')) {
              // Benign race — another start won and is live; skip ERROR/alert.
              setStatus(RecordingStatus.RECORDING);
            } else {
              const isRuntimeError = isTranscriptionRuntimeStartError(error);
              setStatus(RecordingStatus.ERROR, isRuntimeError
                ? TRANSCRIPTION_RUNTIME_USER_MESSAGE
                : errorMsg);
              if (!isRuntimeError) {
                toast.error(tr('toast.startRecordingFailed'), { description: errorMsg });
              }
            }
          } finally {
            setIsAutoStarting(false);
          }
        }
      }
    };

    checkAutoStartRecording();
  }, [
    isRecording,
    isAutoStarting,
    selectedDevices,
    selectedWindowIsStillOpen,
    generateMeetingTitle,
    setMeetingTitle,
    setIsRecording,
    clearTranscripts,
    setIsMeetingActive,
    checkModelReady,
    checkIfModelDownloading,
    showModal,
    setStatus,
  ]);

  // Listen for direct recording trigger from sidebar when already on home page
  useEffect(() => {
    const handleDirectStart = async () => {
      if (isRecording || isAutoStarting) {
        console.log('Recording already in progress, ignoring direct start event');
        return;
      }

      console.log('Direct start from sidebar - checking selected transcription model status');
      setIsAutoStarting(true);

      // Check the selected transcription model before starting.
      const modelReady = await checkModelReady();
      if (!modelReady) {
        const isDownloading = await checkIfModelDownloading();
        if (isDownloading) {
          toast.info(tr('toast.modelDownloading'), {
            description: tr('toast.modelDownloadingHint'),
            duration: 5000,
          });
        } else {
          toast.error(tr('toast.modelNotReady'), {
            description: tr('toast.modelNotReadyHint'),
            duration: 5000,
          });
          showModal?.('modelSelector', 'Transcription model setup required');
        }
        setStatus(RecordingStatus.IDLE);
        setIsAutoStarting(false);
        return;
      }

      try {
        if (!(await selectedWindowIsStillOpen())) {
          setStatus(RecordingStatus.IDLE);
          return;
        }

        // Generate meeting title
        const generatedMeetingTitle = generateMeetingTitle();

        // Set STARTING status before initiating backend recording
        setStatus(RecordingStatus.STARTING, 'Initializing recording...');

        console.log('Starting backend recording with meeting:', generatedMeetingTitle);
        const result = await recordingService.startRecordingWithDevices(
          selectedDevices?.micDevice || null,
          selectedDevices?.systemDevice || null,
          generatedMeetingTitle,
          selectedWindow?.process_id ?? null
        );
        console.log('Backend recording result:', result);

        // Update UI state after successful backend start
        // Note: RECORDING status will be set by RecordingStateContext event listener
        setMeetingTitle(generatedMeetingTitle);
        setIsRecording(true);
        clearTranscripts();
        setIsMeetingActive(true);

        // Show recording notification if enabled
        await showRecordingNotification();
      } catch (error) {
        console.error('Failed to start recording from sidebar:', error);
        const errorMsg = error instanceof Error ? error.message : String(error);
        if (errorMsg.includes('already in progress')) {
          // Benign race — another start won and is live; skip ERROR/alert.
          setStatus(RecordingStatus.RECORDING);
        } else {
          const isRuntimeError = isTranscriptionRuntimeStartError(error);
          setStatus(RecordingStatus.ERROR, isRuntimeError
            ? TRANSCRIPTION_RUNTIME_USER_MESSAGE
            : errorMsg);
          if (!isRuntimeError) {
            toast.error(tr('toast.startRecordingFailed'), { description: errorMsg });
          }
        }
      } finally {
        setIsAutoStarting(false);
      }
    };

    window.addEventListener('start-recording-from-sidebar', handleDirectStart);

    return () => {
      window.removeEventListener('start-recording-from-sidebar', handleDirectStart);
    };
  }, [
    isRecording,
    isAutoStarting,
    selectedDevices,
    selectedWindowIsStillOpen,
    generateMeetingTitle,
    setMeetingTitle,
    setIsRecording,
    clearTranscripts,
    setIsMeetingActive,
    checkModelReady,
    checkIfModelDownloading,
    showModal,
    setStatus,
  ]);

  return {
    handleRecordingStart,
    isAutoStarting,
  };
}
