import { useState, useEffect } from 'react';
import { recordingService } from '@/services/recordingService';

interface UseRecordingStateSyncReturn {
  isBackendRecording: boolean;
  isRecordingDisabled: boolean;
  setIsRecordingDisabled: (value: boolean) => void;
}

/**
 * Custom hook for synchronizing frontend recording state with backend.
 * Polls backend every 1 second to detect recording state changes.
 *
 * Features:
 * - Backend state synchronization (1-second polling)
 * - Recording disabled flag management (prevents re-recording during processing)
 */
export function useRecordingStateSync(
  isRecording: boolean,
  setIsRecording: (value: boolean) => void,
  setIsMeetingActive: (value: boolean) => void
): UseRecordingStateSyncReturn {
  const [isRecordingDisabled, setIsRecordingDisabled] = useState(false);

  useEffect(() => {
    const checkRecordingState = async () => {
      try {
        const isCurrentlyRecording = await recordingService.isRecording();
        if (isCurrentlyRecording && !isRecording) {
          setIsRecording(true);
          setIsMeetingActive(true);
        } else if (!isCurrentlyRecording && isRecording) {
          setIsRecording(false);
        }
      } catch (error) {
        console.error('Failed to check recording state:', error);
      }
    };

    if (typeof window !== 'undefined' && (window as any).__TAURI__) {
      void checkRecordingState();
      // Low-frequency fallback poll: events drive normal updates; this only repairs
      // a missed event (e.g. the app was backgrounded for a long time).
      const interval = setInterval(checkRecordingState, 5000);
      return () => clearInterval(interval);
    }
  }, [isRecording, setIsRecording, setIsMeetingActive]);

  return {
    isBackendRecording: isRecording,
    isRecordingDisabled,
    setIsRecordingDisabled,
  };
}
