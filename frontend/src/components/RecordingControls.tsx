'use client';
import { useI18n } from '@/contexts/I18nContext';

import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { useCallback, useEffect, useState, useRef } from 'react';
import { Play, Pause, Square, Mic, AlertCircle, X } from 'lucide-react';
import { ProcessRequest, SummaryResponse } from '@/types/summary';
import { listen } from '@tauri-apps/api/event';
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { toast } from 'sonner';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import type { TranscriptionErrorPayload } from '@/services/transcriptService';
import { RecordingLevelMonitor } from '@/components/RecordingLevelMonitor';
import { usePrewarmSpeechModel } from '@/hooks/usePrewarmSpeechModel';
import {
  useRecordingLevelMonitoring,
  formatDuration,
  type RecordingLimits,
} from '@/hooks/useRecordingLevelMonitoring';

interface RecordingControlsProps {
  isRecording: boolean;
  barHeights: string[];
  onRecordingStop: (callApi?: boolean) => void;
  onRecordingStart: () => void;
  onTranscriptReceived: (summary: SummaryResponse) => void;
  onStopInitiated?: () => void; // Called immediately when stop button is clicked
  isRecordingDisabled: boolean;
  isParentProcessing: boolean;
  selectedDevices?: {
    micDevice: string | null;
    systemDevice: string | null;
  };
  meetingName?: string;
}

export const RecordingControls: React.FC<RecordingControlsProps> = ({
  isRecording,
  barHeights,
  onRecordingStop,
  onRecordingStart,
  onTranscriptReceived,
  onStopInitiated,
  isRecordingDisabled,
  isParentProcessing,
  selectedDevices,
  meetingName,
}) => {
  const { t } = useI18n();
  // Use global recording state context for pause state (syncs with tray operations)
  const recordingState = useRecordingState();
  const isPaused = recordingState.isPaused;
  const isStartingRecording = recordingState.isStartingRecording;

  const [showPlayback, setShowPlayback] = useState(false);
  const [recordingPath, setRecordingPath] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<string>('');
  const [isProcessing, setIsProcessing] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [isPausing, setIsPausing] = useState(false);
  const [isResuming, setIsResuming] = useState(false);
  const MIN_RECORDING_DURATION = 2000; // 2 seconds minimum recording time
  const [transcriptionErrors, setTranscriptionErrors] = useState(0);
  const [isValidatingModel, setIsValidatingModel] = useState(false);
  const [speechDetected, setSpeechDetected] = useState(false);
  const [deviceError, setDeviceError] = useState<{ title: string, message: string } | null>(null);

  // Recording limits, loaded from preferences. Defaults are "off" so the
  // behaviour is unchanged until the user opts in.
  const [recordingLimits, setRecordingLimits] = useState<RecordingLimits>({
    silenceStopSeconds: 0,
    maxRecordingSeconds: 0,
  });

  // The limits are enforced from a hook declared above `handleStopRecording`, so
  // the stop handler is reached through a ref rather than reordering the
  // component's many callbacks.
  const handleRecordingStopRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    invoke<{ silence_stop_enabled_seconds?: number; max_recording_seconds?: number }>(
      'get_recording_preferences',
    )
      .then((prefs) =>
        setRecordingLimits({
          silenceStopSeconds: prefs?.silence_stop_enabled_seconds ?? 0,
          maxRecordingSeconds: prefs?.max_recording_seconds ?? 0,
        }),
      )
      .catch((error) => console.error('Failed to load recording limits:', error));
  }, []);

  // Keep the OS level monitor running only while capture is live, so the meter
  // above has data and the microphone is released when recording stops. The same
  // hook enforces the optional silence and length limits.
  useRecordingLevelMonitoring(
    isRecording,
    [selectedDevices?.micDevice, selectedDevices?.systemDevice].filter(
      (name): name is string => Boolean(name),
    ),
    {
      silenceStopSeconds: recordingLimits.silenceStopSeconds,
      maxRecordingSeconds: recordingLimits.maxRecordingSeconds,
    },
    {
      onLimitReached: (reason) => {
        toast.info(reason);
        // Reuse the normal stop path so the recording is finalized (merged,
        // converted, tagged) exactly as a manual stop would.
        void handleRecordingStopRef.current?.();
      },
      onLimitApproaching: (what, secondsRemaining) => {
        toast.warning(`${what}: stopping in ${formatDuration(secondsRemaining)}`, {
          id: 'recording-limit-warning',
        });
      },
      onDiskSpaceLow: (freeMb) => {
        toast.warning('Low disk space', {
          id: 'recording-disk-warning',
          description: `Only ${freeMb} MB free where recordings are saved. Free up space so the rest of this recording is not lost.`,
        });
      },
    },
  );

  const currentTime = 0;
  const duration = 0;
  const isPlaying = false;
  const progress = 0;

  const formatTime = (time: number) => {
    const minutes = Math.floor(time / 60);
    const seconds = Math.floor(time % 60);
    return `${minutes}:${seconds.toString().padStart(2, '0')}`;
  };

  useEffect(() => {
    const checkTauri = async () => {
      try {
        const result = await invoke('is_recording');
        console.log('Tauri is initialized and ready, is_recording result:', result);
      } catch (error) {
        console.error('Tauri initialization error:', error);
        toast.error(t('toast.initRecordingFailed'), {
          description: error instanceof Error ? error.message : String(error),
        });
      }
    };
    checkTauri();
  }, []);

  const prewarmHandlers = usePrewarmSpeechModel();

  const handleStartRecording = useCallback(async () => {
    if (isStarting || isValidatingModel || isStartingRecording) return;
    console.log('Starting recording...');
    console.log('Selected devices:', selectedDevices);
    console.log('Meeting name:', meetingName);
    console.log('Current isRecording state:', isRecording);

    setShowPlayback(false);
    setTranscript(''); // Clear any previous transcript
    setSpeechDetected(false); // Reset speech detection on new recording

    try {
      // Call the validation callback which will:
      // 1. Check if model is ready
      // 2. Show appropriate toast/modal
      // 3. Call backend if valid
      // 4. Update UI state
      await onRecordingStart();
    } catch (error) {
      console.error('Failed to start recording:', error);
      console.error('Error details:', {
        message: error instanceof Error ? error.message : String(error),
        name: error instanceof Error ? error.name : 'Unknown',
        stack: error instanceof Error ? error.stack : undefined
      });

      // Parse error message to provide user-friendly feedback
      const errorMsg = error instanceof Error ? error.message : String(error);

      // Check for device-related errors
      if (errorMsg.includes('microphone') || errorMsg.includes('mic') || errorMsg.includes('input')) {
        setDeviceError({
          title: 'Microphone Not Available',
          message: 'Unable to access your microphone. Please check that:\n• Your microphone is connected\n• The app has microphone permissions\n• No other app is using the microphone'
        });
      } else if (errorMsg.includes('system audio') || errorMsg.includes('speaker') || errorMsg.includes('output')) {
        setDeviceError({
          title: 'System Audio Not Available',
          message: 'Unable to capture system audio. Please check that:\n• A virtual audio device (like BlackHole) is installed\n• The app has screen recording permissions (macOS)\n• System audio is properly configured'
        });
      } else if (errorMsg.includes('permission')) {
        setDeviceError({
          title: 'Permission Required',
          message: 'Recording permissions are required. Please:\n• Grant microphone access in System Settings\n• Grant screen recording access for system audio (macOS)\n• Restart the app after granting permissions'
        });
      } else {
        setDeviceError({
          title: 'Recording Failed',
          message: 'Unable to start recording. Please check your audio device settings and try again.'
        });
      }
    }
  }, [onRecordingStart, isStarting, isValidatingModel, selectedDevices, meetingName, isRecording]);

  const stopRecordingAction = useCallback(async () => {
    console.log('Executing stop recording...');
    try {
      setIsProcessing(true);
      const dataDir = await appDataDir();
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
      const savePath = `${dataDir}/recording-${timestamp}.wav`;
      console.log('Saving recording to:', savePath);
      console.log('About to call stop_recording command');
      const result = await invoke('stop_recording', {
        args: {
          save_path: savePath
        }
      });
      console.log('stop_recording command completed successfully:', result);
      setRecordingPath(savePath);
      // setShowPlayback(true);
      setIsProcessing(false);
      // Track successful transcription
      onRecordingStop(true);
    } catch (error) {
      console.error('Failed to stop recording:', error);
      if (error instanceof Error) {
        console.error('Error details:', {
          message: error.message,
          name: error.name,
          stack: error.stack,
        });
        if (error.message.includes('No recording in progress')) {
          return;
        }
      } else if (typeof error === 'string' && error.includes('No recording in progress')) {
        return;
      } else if (error && typeof error === 'object' && 'toString' in error) {
        if (error.toString().includes('No recording in progress')) {
          return;
        }
      }
      setIsProcessing(false);
      onRecordingStop(false);
    } finally {
      setIsStopping(false);
    }
  }, [onRecordingStop]);

  const handleStopRecording = useCallback(async () => {
    console.log('handleStopRecording called - isRecording:', isRecording, 'isStarting:', isStarting, 'isStopping:', isStopping, 'isStartingRecording:', isStartingRecording);
    if (!isRecording || isStarting || isStopping || isStartingRecording) {
      console.log('Early return from handleStopRecording due to state check');
      return;
    }

    console.log('Stopping recording...');

    // Notify parent immediately (for UI state updates)
    onStopInitiated?.();

    setIsStopping(true);

    // Immediately trigger the stop action
    await stopRecordingAction();
  }, [isRecording, isStarting, isStopping, isStartingRecording, stopRecordingAction, onStopInitiated]);

  // Expose the stop handler to the level-monitoring hook, which may fire a limit
  // stop before this callback has been recreated.
  handleRecordingStopRef.current = handleStopRecording;

  const handlePauseRecording = useCallback(async () => {
    if (!isRecording || isPaused || isPausing) return;

    console.log('Pausing recording...');
    setIsPausing(true);

    try {
      await invoke('pause_recording');
      // isPaused state now managed by RecordingStateContext via events
      console.log('Recording paused successfully');
    } catch (error) {
      console.error('Failed to pause recording:', error);
      toast.error(t('toast.pauseRecordingFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setIsPausing(false);
    }
  }, [isRecording, isPaused, isPausing]);

  const handleResumeRecording = useCallback(async () => {
    if (!isRecording || !isPaused || isResuming) return;

    console.log('Resuming recording...');
    setIsResuming(true);

    try {
      await invoke('resume_recording');
      // isPaused state now managed by RecordingStateContext via events
      console.log('Recording resumed successfully');
    } catch (error) {
      console.error('Failed to resume recording:', error);
      toast.error(t('toast.resumeRecordingFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setIsResuming(false);
    }
  }, [isRecording, isPaused, isResuming]);

  useEffect(() => {
    return () => {
      // Cleanup on unmount if needed
    };
  }, []);

  useEffect(() => {
    console.log('Setting up recording event listeners');
    let unsubscribes: (() => void)[] = [];

    const setupListeners = async () => {
      try {
        // Transcription error listener - handles structured error objects with actionable flag
        const transcriptionErrorUnsubscribe = await listen<TranscriptionErrorPayload>('transcription-error', (event) => {
          console.log('transcription-error event received:', event);
          console.error('Transcription error received:', event.payload);

          const errorMessage = event.payload.userMessage || event.payload.error;

          console.log('Tracked transcription error:', errorMessage);

          setTranscriptionErrors(prev => {
            const newCount = prev + 1;
            console.log('Transcription error count incremented:', newCount);
            return newCount;
          });
          setIsProcessing(false);

          if (event.payload.phase === 'active') {
            console.log('Calling onRecordingStop(false) due to active transcription error');
            onRecordingStop(false);
          }

          // For actionable errors (like model loading failures), the main page will handle showing the model selector
          // For regular errors, they are handled by useModalState global listener which shows a toast
        });

        // Pause/Resume events are now handled by RecordingStateContext
        // No need for duplicate listeners here

        // Speech detected listener - for UX feedback when VAD detects speech
        const speechDetectedUnsubscribe = await listen('speech-detected', (event) => {
          console.log('speech-detected event received:', event);
          setSpeechDetected(true);
        });

        // Provider auto-switch notice: the configured engine does not support the
        // selected language, so a compatible one is used for this recording.
        // Without this the user would only notice that transcripts look different.
        const providerSwitchedUnsubscribe = await listen<{
          fromLabel?: string;
          toLabel?: string;
          language?: string | null;
          userMessage?: string;
        }>('transcription-provider-switched', (event) => {
          const { fromLabel, toLabel, language, userMessage } = event.payload ?? {};
          console.log('transcription-provider-switched event received:', event);

          toast.info('Transcription engine changed for this recording', {
            description:
              userMessage
              ?? `${fromLabel ?? 'The selected engine'} does not support ${
                language ? `"${language}"` : 'this language'
              }, so ${toLabel ?? 'another engine'} is being used.`,
            duration: 8000,
          });
        });

        unsubscribes = [
          transcriptionErrorUnsubscribe,
          speechDetectedUnsubscribe,
          providerSwitchedUnsubscribe
        ];
        console.log('Recording event listeners set up successfully');
      } catch (error) {
        console.error('Failed to set up recording event listeners:', error);
      }
    };

    setupListeners();

    return () => {
      console.log('Cleaning up recording event listeners');
      unsubscribes.forEach(unsubscribe => {
        if (unsubscribe && typeof unsubscribe === 'function') {
          unsubscribe();
        }
      });
    };
  }, [onRecordingStop]);

  return (
    <TooltipProvider>
      <div className="flex flex-col space-y-2">
        <div className="flex items-center space-x-2 bg-white rounded-full shadow-lg px-4 py-2">
          {isProcessing && !isParentProcessing ? (
            <div className="flex items-center space-x-2">
              <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-gray-900"></div>
              <span className="text-sm text-gray-600">Processing recording...</span>
            </div>
          ) : (
            <>
              {showPlayback ? (
                <>
                  <button
                    onClick={handleStartRecording}
                    className="w-10 h-10 flex items-center justify-center bg-red-500 rounded-full text-white hover:bg-red-600 transition-colors"
                  >
                    <Mic size={16} />
                  </button>

                  <div className="w-px h-6 bg-gray-200 mx-1" />

                  <div className="flex items-center space-x-1 mx-2">
                    <div className="text-sm text-gray-600 min-w-[40px]">
                      {formatTime(currentTime)}
                    </div>
                    <div
                      className="relative w-24 h-1 bg-gray-200 rounded-full"
                    >
                      <div
                        className="absolute h-full bg-blue-500 rounded-full"
                        style={{ width: `${progress}%` }}
                      />
                    </div>
                    <div className="text-sm text-gray-600 min-w-[40px]">
                      {formatTime(duration)}
                    </div>
                  </div>

                  <button
                    className="w-10 h-10 flex items-center justify-center bg-gray-300 rounded-full text-white cursor-not-allowed"
                    disabled
                  >
                    <Play size={16} />
                  </button>
                </>
              ) : (
                <>
                  {!isRecording ? (
                    // Start recording button
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          {...prewarmHandlers}
                          onClick={() => {
                            handleStartRecording();
                          }}
                          disabled={isStarting || isProcessing || isRecordingDisabled || isValidatingModel || isStartingRecording}
                          className={`w-12 h-12 flex items-center justify-center ${isStarting || isProcessing || isValidatingModel || isStartingRecording ? 'bg-gray-400' : 'bg-red-500 hover:bg-red-600'
                            } rounded-full text-white transition-colors relative`}
                        >
                          {isValidatingModel || isStartingRecording ? (
                            <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-white"></div>
                          ) : (
                            <Mic size={20} />
                          )}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>{t('nav.startRecording')}</p>
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    // Recording controls (pause/resume + stop)
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => {
                              if (isPaused) {
                                handleResumeRecording();
                              } else {
                                handlePauseRecording();
                              }
                            }}
                            disabled={isPausing || isResuming || isStopping}
                            className={`w-10 h-10 flex items-center justify-center ${isPausing || isResuming || isStopping
                              ? 'bg-gray-200 border-2 border-gray-300 text-gray-400'
                              : 'bg-white border-2 border-gray-300 text-gray-600 hover:border-gray-400 hover:bg-gray-50'
                              } rounded-full transition-colors relative`}
                          >
                            {isPaused ? <Play size={16} /> : <Pause size={16} />}
                            {(isPausing || isResuming) && (
                              <div className="absolute -top-8 text-gray-600 font-medium text-xs">
                                {isPausing ? t('live.pausing') : t('live.resuming')}
                              </div>
                            )}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>{isPaused ? t('live.resume') : t('live.pause')}</p>
                        </TooltipContent>
                      </Tooltip>

                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => {
                              handleStopRecording();
                            }}
                            disabled={isStopping || isPausing || isResuming || isStartingRecording}
                            className={`w-10 h-10 flex items-center justify-center ${isStopping || isPausing || isResuming || isStartingRecording ? 'bg-gray-400' : 'bg-red-500 hover:bg-red-600'
                              } rounded-full text-white transition-colors relative`}
                          >
                            <Square size={16} />
                            {isStopping && (
                              <div className="absolute -top-8 text-gray-600 font-medium text-xs">
                                Stopping...
                              </div>
                            )}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>{t('live.stop')}</p>
                        </TooltipContent>
                      </Tooltip>
                    </>
                  )}

                  <div className="flex items-center space-x-1 mx-4">
                    {barHeights.map((height, index) => (
                      <div
                        key={index}
                        className={`w-1 rounded-full transition-all duration-200 ${isPaused ? 'bg-orange-500' : 'bg-red-500'
                          }`}
                        style={{
                          height: isRecording && !isPaused ? height : '4px',
                          opacity: isPaused ? 0.6 : 1,
                        }}
                      />
                    ))}
                  </div>

                  {/* Live input levels. Placed next to the recording indicator so a
                      silent microphone or wrong output device is visible while it
                      still matters, rather than after the meeting. */}
                  <RecordingLevelMonitor
                    active={isRecording && !isPaused}
                    className="mx-4 min-w-[16rem]"
                  />
                </>
              )}
            </>
          )}
        </div>

        {/* Show validation status only */}
        {isValidatingModel && (
          <div className="text-xs text-gray-600 text-center mt-2">
            Validating speech recognition...
          </div>
        )}

        {/* Device error alert */}
        {deviceError && (
          <Alert variant="destructive" className="mt-4 border-red-300 bg-red-50">
            <AlertCircle className="h-5 w-5 text-red-600" />
            <button
              onClick={() => setDeviceError(null)}
              className="absolute right-3 top-3 text-red-600 hover:text-red-800 transition-colors"
              aria-label="Close alert"
            >
              <X className="h-4 w-4" />
            </button>
            <AlertTitle className="text-red-800 font-semibold mb-2">
              {deviceError.title}
            </AlertTitle>
            <AlertDescription className="text-red-700">
              {deviceError.message.split('\n').map((line, i) => (
                <div key={i} className={i > 0 ? 'ml-2' : ''}>
                  {line}
                </div>
              ))}
            </AlertDescription>
          </Alert>
        )}

        {/* {showPlayback && recordingPath && (
        <div className="text-sm text-gray-600 px-4">
          Recording saved to: {recordingPath}
        </div>
      )} */}
      </div>
    </TooltipProvider>
  );
};