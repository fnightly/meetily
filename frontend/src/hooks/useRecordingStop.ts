import { useState, useEffect, useCallback, useRef } from 'react';
import { tr } from '@/lib/i18n';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { storageService } from '@/services/storageService';
import { transcriptService } from '@/services/transcriptService';
import {
  actionNeedsSummary,
  actionNeedsTranscription,
  type PostRecordingAction,
} from '@/lib/post-recording';
import { batchTranscriptionTarget, type ActiveSpeechSettings } from '@/lib/transcription-target';
import type { TranscriptSegmentData } from '@/types';
import {
  applyPinnedSummaryLanguageToMeeting,
  detectAndCacheSummaryLanguage,
} from '@/lib/summary-language-preferences';

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';

interface UseRecordingStopReturn {
  handleRecordingStop: (callApi: boolean) => Promise<void>;
  isStopping: boolean;
  isProcessingTranscript: boolean;
  isSavingTranscript: boolean;
  summaryStatus: SummaryStatus;
  setIsStopping: (value: boolean) => void;
  /** Recording awaiting a transcription decision, or null. */
  pendingPostRecording: { meetingId: string } | null;
  /** Apply the user's choice for the pending recording. */
  handlePostRecordingChoice: (action: PostRecordingAction) => Promise<void>;
  /** Live progress while the post-recording choice runs, when it transcribes. */
  postRecordingProgress: { percentage: number; stage: string } | null;
}

/**
 * Custom hook for managing recording stop lifecycle.
 * Handles the complex stop sequence: transcription wait → buffer flush → SQLite save → navigation.
 *
 * Features:
 * - Transcription completion polling (60s max, 500ms interval)
 * - Transcript buffer flush coordination
 * - SQLite meeting save with folder_path from sessionStorage
 * - Auto-navigation to meeting details
 * - Toast notifications for success/error
 * - Window exposure for Rust callbacks
 */
export function useRecordingStop(
  setIsRecording: (value: boolean) => void,
  setIsRecordingDisabled: (value: boolean) => void
): UseRecordingStopReturn {
  // USE global state instead
  const recordingState = useRecordingState();
  const {
    status,
    setStatus,
    isStopping,
    isProcessing: isProcessingTranscript,
    isSaving: isSavingTranscript
  } = recordingState;

  /**
   * A saved recording awaiting the user's decision about transcription.
   *
   * Non-null while the post-recording dialog should be shown.
   */
  const [pendingPostRecording, setPendingPostRecording] = useState<{
    meetingId: string;
  } | null>(null);
  const [postRecordingProgress, setPostRecordingProgress] = useState<{
    percentage: number;
    stage: string;
  } | null>(null);

  const {
    transcriptsRef,
    flushBuffer,
    clearTranscripts,
    meetingTitle,
    markMeetingAsSaved,
  } = useTranscripts();

  const {
    refetchMeetings,
    setCurrentMeeting,
    setMeetings,
    meetings,
    setIsMeetingActive,
  } = useSidebar();

  const router = useRouter();

  // Guard to prevent duplicate/concurrent stop calls (e.g., from UI and tray simultaneously)
  const stopInProgressRef = useRef(false);

  // Promise to track recording-stopped event data (fixes race condition with recording-stop-complete)
  const recordingStoppedDataRef = useRef<Promise<void> | null>(null);

  // Set up recording-stopped listener for meeting navigation
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;

    const setupRecordingStoppedListener = async () => {
      try {
        console.log('Setting up recording-stopped listener for navigation...');
        unlistenFn = await listen<{
          message: string;
          folder_path?: string;
          meeting_name?: string;
          transcription_deferred?: boolean;
        }>('recording-stopped', async (event) => {
          // Create promise that resolves when sessionStorage is set (prevents race condition)
          recordingStoppedDataRef.current = (async () => {
            const { folder_path, meeting_name, transcription_deferred } = event.payload;

            // Store folder_path and meeting_name for later use in handleRecordingStop
            if (folder_path) {
              sessionStorage.setItem('last_recording_folder_path', folder_path);
            }
            if (meeting_name) {
              sessionStorage.setItem('last_recording_meeting_name', meeting_name);
            }
            // Recorded per session, not read from preferences at stop time: the
            // user may have changed the setting while recording, and the stop path
            // must reflect what this recording actually did.
            sessionStorage.setItem(
              'last_recording_deferred',
              transcription_deferred ? 'true' : 'false',
            );
          })();

        });
        console.log('Recording stopped listener setup complete');
      } catch (error) {
        console.error('Failed to setup recording stopped listener:', error);
      }
    };

    setupRecordingStoppedListener();

    return () => {
      console.log('Cleaning up recording stopped listener...');
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [router]);

  /**
   * Navigate to a saved meeting.
   *
   * Shared by the normal path and the post-recording choice path so both land in
   * the same place.
   */
  const navigateToMeeting = useCallback(
    (meetingId: string) => {
      router.push(`/meeting-details?id=${meetingId}&source=recording`);
      clearTranscripts();
      setStatus(RecordingStatus.IDLE);
    },
    [router, clearTranscripts, setStatus],
  );

  /**
   * Rewrite the transcript with a local model, when the user has opted in.
   *
   * Cleanup removes filler words and fixes spoken punctuation, which means it
   * rewrites the user's own words — so it is off unless enabled, and the text is
   * re-split into the existing segments rather than replacing them wholesale so
   * playback timings survive.
   *
   * Any failure keeps the original transcript. Losing a meeting's text because a
   * cleanup model was unavailable would be far worse than storing it uncleaned.
   */
  const applyTranscriptCleanup = useCallback(
    // Generic over the element type so the caller's own shape (with its string or
    // numeric timestamp) survives; only `text` is ever rewritten.
    async <T extends { text: string }>(segments: T[]): Promise<T[]> => {
      if (segments.length === 0) return segments;

      try {
        const prefs = await invoke<{ cleanup_after_recording?: boolean }>(
          'get_recording_preferences',
        );
        if (!prefs?.cleanup_after_recording) {
          console.log('Transcript cleanup is disabled; keeping the raw transcript');
          return segments;
        }
      } catch (error) {
        console.warn('Could not read the cleanup preference; skipping cleanup:', error);
        return segments;
      }

      try {
        setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Cleaning up the transcript...');

        const joined = segments.map((segment) => segment.text).join('\n');
        const outcome = await invoke<{
          cleaned: string | null;
          applied: boolean;
          skipped: string | null;
        }>('cleanup_transcript', { text: joined });

        if (!outcome.applied || !outcome.cleaned) {
          if (outcome.skipped) console.log(`Transcript cleanup skipped: ${outcome.skipped}`);
          return segments;
        }

        // Keep the timing of each original segment: only the text is replaced, so
        // playback sync and the audio offsets stored with each row stay valid.
        const cleanedLines = outcome.cleaned
          .split('\n')
          .map((line) => line.trim())
          .filter((line) => line.length > 0);

        if (cleanedLines.length === 0) {
          console.warn('Cleanup produced no lines; keeping the raw transcript');
          return segments;
        }

        const cleanedSegments = segments.map((segment, index) => ({
          ...segment,
          // A model may merge or split lines; mapping by position keeps the shape
          // predictable, and any surplus text is appended to the last segment
          // rather than dropped.
          text: cleanedLines[index] ?? '',
        }));

        const merged = cleanedSegments
          .map((segment) => segment.text)
          .join(' ')
          .trim();

        if (cleanedLines.length > segments.length) {
          const surplus = cleanedLines.slice(segments.length).join(' ');
          const last = cleanedSegments[cleanedSegments.length - 1];
          last.text = `${last.text} ${surplus}`.trim();
        } else if (!merged) {
          console.warn('Cleanup produced empty segments; keeping the raw transcript');
          return segments;
        }

        // Drop segments the model emptied out, e.g. a line that was pure filler.
        const kept = cleanedSegments.filter((segment) => segment.text.length > 0);
        if (kept.length === 0) return segments;

        console.log(`Transcript cleanup applied: ${segments.length} -> ${kept.length} segments`);
        return kept;
      } catch (error) {
        console.error('Transcript cleanup failed; keeping the raw transcript:', error);
        toast.warning(tr('toast.cleanupFailed'), {
          description: tr('toast.cleanupFailedHint'),
        });
        return segments;
      }
    },
    [setStatus],
  );

  // Main recording stop handler
  const handleRecordingStop = useCallback(async (isCallApi: boolean) => {
    if (recordingStoppedDataRef.current) {
      await recordingStoppedDataRef.current;
    }

    // Guard: prevent duplicate/concurrent stop calls
    if (stopInProgressRef.current) {
      return;
    }
    stopInProgressRef.current = true;

    // Set status to STOPPING immediately
    setStatus(RecordingStatus.STOPPING);
    setIsRecording(false);
    setIsRecordingDisabled(true);
    const stopStartTime = Date.now();

    try {
      console.log('Post-stop processing (new implementation)...', {
        stop_initiated_at: new Date(stopStartTime).toISOString(),
        current_transcript_count: transcriptsRef.current.length
      });

      // Note: stop_recording is already called by RecordingControls.stopRecordingAction
      // This function only handles post-stop processing (transcription wait, API call, navigation)
      console.log('Recording already stopped by RecordingControls, processing transcription...');

      // Whether this recording skipped realtime transcription. Read from the stop
      // event rather than preferences: the setting may have changed mid-recording.
      const deferred =
        sessionStorage.getItem('last_recording_deferred') === 'true';

      // Wait for transcription to complete
      setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Waiting for transcription...');
      console.log('Waiting for transcription to complete...');

      const MAX_WAIT_TIME = 60000; // 60 seconds maximum wait (increased for longer processing)
      const POLL_INTERVAL = 500; // Check every 500ms
      let elapsedTime = 0;
      // In deferred mode no chunks were ever queued for transcription, so there is
      // nothing to wait for: going through the poll loop would stall the stop
      // sequence for the "no recent activity" grace period on every recording.
      let transcriptionComplete = deferred;

      // Listen for transcription-complete event
      const unlistenComplete = await listen('transcription-complete', () => {
        console.log('Received transcription-complete event');
        transcriptionComplete = true;
      });

      // Poll for transcription status
      while (elapsedTime < MAX_WAIT_TIME && !transcriptionComplete) {
        try {
          const status = await transcriptService.getTranscriptionStatus();
          console.log('Transcription status:', status);

          // Check if transcription is complete
          if (!status.is_processing && status.chunks_in_queue === 0) {
            console.log('Transcription complete - no active processing and no chunks in queue');
            transcriptionComplete = true;
            break;
          }

          // If no activity for more than 8 seconds and no chunks in queue, consider it done (increased from 5s to 8s)
          if (status.last_activity_ms > 8000 && status.chunks_in_queue === 0) {
            console.log('Transcription likely complete - no recent activity and empty queue');
            transcriptionComplete = true;
            break;
          }

          // Update user with current status
          if (status.chunks_in_queue > 0) {
            console.log(`Processing ${status.chunks_in_queue} remaining audio chunks...`);
            setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, `Processing ${status.chunks_in_queue} remaining chunks...`);
          }

          // Wait before next check
          await new Promise(resolve => setTimeout(resolve, POLL_INTERVAL));
          elapsedTime += POLL_INTERVAL;
        } catch (error) {
          console.error('Error checking transcription status:', error);
          break;
        }
      }

      // Clean up listener
      console.log('🧹 CLEANUP: Cleaning up transcription-complete listener');
      unlistenComplete();

      if (deferred) {
        // No live transcription happened, so there are no late segments to wait
        // for and nothing in the buffer to flush.
        console.log('⏩ Transcription was deferred; skipping the post-wait grace period');
      } else if (!transcriptionComplete && elapsedTime >= MAX_WAIT_TIME) {
        console.warn('⏰ Transcription wait timeout reached after', elapsedTime, 'ms');
      } else {
        console.log('✅ Transcription completed after', elapsedTime, 'ms');
        // Wait longer for any late transcript segments (increased from 1s to 4s)
        console.log('⏳ Waiting for late transcript segments...');
        await new Promise(resolve => setTimeout(resolve, 4000));
      }

      // Final buffer flush: process ALL remaining transcripts regardless of timing
      const flushStartTime = Date.now();
      console.log('🔄 Final buffer flush: forcing processing of any remaining transcripts...', {
        flush_started_at: new Date(flushStartTime).toISOString(),
        time_since_stop: flushStartTime - stopStartTime,
        current_transcript_count: transcriptsRef.current.length
      });
      setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Flushing transcript buffer...');
      flushBuffer();
      const flushEndTime = Date.now();
      console.log('✅ Final buffer flush completed', {
        flush_duration: flushEndTime - flushStartTime,
        total_time_since_stop: flushEndTime - stopStartTime,
        final_transcript_count: transcriptsRef.current.length
      });

      // NOTE: Status remains PROCESSING_TRANSCRIPTS until we start saving

      // Wait a bit more to ensure all transcript state updates have been processed
      console.log('Waiting for transcript state updates to complete...');
      await new Promise(resolve => setTimeout(resolve, 500));

      // Save to SQLite
      // NOTE: enabled to save COMPLETE transcripts after frontend receives all updates
      // This ensures user sees all transcripts streaming in before database save
      //
      // In deferred mode the meeting is saved even though it has no transcript rows:
      // without a meeting record the audio on disk is unreachable from the UI, so it
      // could never be transcribed afterwards. An empty transcript list is a valid
      // state for a saved recording.
      if (isCallApi && (transcriptionComplete || deferred)) {

        setStatus(RecordingStatus.SAVING, 'Saving meeting to database...');

        // Get fresh transcript state (ALL transcripts including late ones)
        let freshTranscripts = [...transcriptsRef.current];

        // Cleanup runs before saving so the database stores the cleaned text and
        // the user never sees the raw version replaced afterwards.
        freshTranscripts = await applyTranscriptCleanup(freshTranscripts);

        // Get folder_path and meeting_name from recording-stopped event
        const folderPath = sessionStorage.getItem('last_recording_folder_path');
        const savedMeetingName = sessionStorage.getItem('last_recording_meeting_name');

        console.log('💾 Saving COMPLETE transcripts to database...', {
          transcript_count: freshTranscripts.length,
          meeting_name: savedMeetingName || meetingTitle,
          folder_path: folderPath,
          sample_text: freshTranscripts.length > 0 ? freshTranscripts[0].text.substring(0, 50) + '...' : 'none',
          last_transcript: freshTranscripts.length > 0 ? freshTranscripts[freshTranscripts.length - 1].text.substring(0, 30) + '...' : 'none',
        });

        try {
          const responseData = await storageService.saveMeeting(
            savedMeetingName || meetingTitle || 'New Meeting',  // PREFER savedMeetingName (backend source)
            freshTranscripts,
            folderPath
          );

          const meetingId = responseData.meeting_id;
          if (!meetingId) {
            console.error('No meeting_id in response:', responseData);
            throw new Error('No meeting ID received from save operation');
          }

          let shouldDetectSummaryLanguage = false;
          try {
            shouldDetectSummaryLanguage = !(await applyPinnedSummaryLanguageToMeeting(meetingId));
          } catch (error) {
            console.warn('Failed to apply pinned summary language preference for new meeting:', error);
            toast.warning(tr('toast.defaultLangFailed'), {
              description: tr('toast.defaultLangFailedHint'),
            });
          }

          if (shouldDetectSummaryLanguage) {
            try {
              await detectAndCacheSummaryLanguage(
                meetingId,
                freshTranscripts.map(t => t.text)
              );
            } catch (error) {
              console.warn('Failed to detect summary language for new meeting:', error);
              toast.warning(tr('toast.detectLangFailed'), {
                description: tr('toast.detectLangFailedHint'),
              });
            }
          }

          console.log('✅ Successfully saved COMPLETE meeting with ID:', meetingId);
          console.log('   Transcripts:', freshTranscripts.length);
          console.log('   folder_path:', folderPath);

          // Mark meeting as saved in IndexedDB (for recovery system)
          await markMeetingAsSaved();

          // Clean up session storage
          sessionStorage.removeItem('last_recording_folder_path');
          sessionStorage.removeItem('last_recording_meeting_name');
          // Clean up IndexedDB meeting ID (redundant with markMeetingAsSaved cleanup, but ensures cleanup)
          sessionStorage.removeItem('indexeddb_current_meeting_id');

          // Refetch meetings and set current meeting
          await refetchMeetings();

          try {
            const meetingData = await storageService.getMeeting(meetingId);
            if (meetingData) {
              setCurrentMeeting({
                id: meetingId,
                title: meetingData.title
              });
              console.log('✅ Current meeting set:', meetingData.title);
            }
          } catch (error) {
            console.warn('Could not fetch meeting details, using ID only:', error);
            setCurrentMeeting({ id: meetingId, title: savedMeetingName || meetingTitle || 'New Meeting' });
          }

          // Mark as completed
          setStatus(RecordingStatus.COMPLETED);

          if (deferred) {
            // Deferred transcription has no live text, so the sensible next step is
            // a choice rather than a silent save. Ask, then act on the answer.
            toast.success(tr('toast.recordingSaved'), {
              description: tr('toast.recordingSavedChoose'),
            });
            setPendingPostRecording({ meetingId });
          } else {
            // Show success toast with navigation option
            toast.success(tr('toast.recordingSavedOk'), {
              description: tr('toast.segmentsSaved', { count: freshTranscripts.length }),
              action: {
                label: tr('toast.viewMeeting'),
                onClick: () => {
                  router.push(`/meeting-details?id=${meetingId}`);
                }
              },
              duration: 10000,
            });

            // Auto-navigate after a short delay with source parameter
            setTimeout(() => navigateToMeeting(meetingId), 2000);
          }
        } catch (saveError) {
          console.error('Failed to save meeting to database:', saveError);
          setStatus(RecordingStatus.ERROR, saveError instanceof Error ? saveError.message : tr('toast.unknownError'));
          toast.error(tr('toast.saveMeetingFailed'), {
            description: saveError instanceof Error ? saveError.message : tr('toast.unknownError')
          });
          throw saveError;
        }
      } else {
        // No save needed, go back to IDLE
        setStatus(RecordingStatus.IDLE);
      }

      setIsMeetingActive(false);
      // isRecording already set to false at function start
      setIsRecordingDisabled(false);
    } catch (error) {
      console.error('Error in handleRecordingStop:', error);
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : tr('toast.unknownError'));
      // isRecording already set to false at function start
      setIsRecordingDisabled(false);
    } finally {
      // Always reset the guard flag when done
      stopInProgressRef.current = false;
    }
  }, [
    setIsRecording,
    setIsRecordingDisabled,
    setStatus,
    transcriptsRef,
    flushBuffer,
    clearTranscripts,
    meetingTitle,
    markMeetingAsSaved,
    refetchMeetings,
    setCurrentMeeting,
    setMeetings,
    meetings,
    setIsMeetingActive,
    router,
    navigateToMeeting,
    setPendingPostRecording,
  ]);

  /**
   * Wait until a retranscription finishes.
   *
   * `start_retranscription_command` returns as soon as the work is queued, so the
   * completion event is what marks it done. Bounded by a timeout so a lost event
   * still lets the UI finish rather than hanging on the dialog.
   */
  const waitForRetranscription = useCallback(
    async (
      meetingId: string,
      onProgress?: (progress: { percentage: number; stage: string }) => void,
      timeoutMs = 30 * 60 * 1000,
    ): Promise<void> => {
      await new Promise<void>((resolve) => {
        let settled = false;
        let unlistenComplete: (() => void) | undefined;
        let unlistenProgress: (() => void) | undefined;

        const finish = () => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          unlistenComplete?.();
          unlistenProgress?.();
          resolve();
        };

        const timer = setTimeout(() => {
          console.warn('Retranscription wait timed out; continuing');
          finish();
        }, timeoutMs);

        listen<{ meeting_id?: string; progress_percentage?: number; stage?: string }>(
          'retranscription-progress',
          (event) => {
            if (event.payload?.meeting_id && event.payload.meeting_id !== meetingId) return;
            onProgress?.({
              percentage: Math.max(0, Math.min(100, Math.round(event.payload?.progress_percentage ?? 0))),
              stage: event.payload?.stage ?? '',
            });
          },
        )
          .then((fn) => {
            if (settled) fn();
            else unlistenProgress = fn;
          })
          .catch((error) => console.error('Failed to subscribe to retranscription-progress:', error));

        listen<{ meeting_id?: string }>('retranscription-complete', (event) => {
          // Only react to this meeting's completion.
          if (!event.payload?.meeting_id || event.payload.meeting_id === meetingId) {
            finish();
          }
        })
          .then((fn) => {
            if (settled) fn();
            else unlistenComplete = fn;
          })
          .catch((error) => {
            console.error('Failed to subscribe to retranscription-complete:', error);
            finish();
          });
      });
    },
    [],
  );

  // The batch command falls back to Whisper when it is not told which engine to
  // use; forward the active speech settings so a Parakeet or cloud choice is used.
  const resolveBatchTarget = useCallback(async () => {
    try {
      return batchTranscriptionTarget(await invoke<ActiveSpeechSettings>('api_get_active_speech_settings'));
    } catch (error) {
      console.error('Failed to read the active speech settings:', error);
      return batchTranscriptionTarget(null);
    }
  }, []);

  /**
   * Apply the user's choice for a recording that has no live transcript.
   *
   * `keep` is the do-nothing path: the audio is already saved and the meeting can
   * be transcribed later from its details page.
   */
  const handlePostRecordingChoice = useCallback(
    async (action: PostRecordingAction) => {
      const pending = pendingPostRecording;
      if (!pending) return;

      // Keep the dialog open while the work runs: closing it immediately made the
      // busy state and progress invisible, so a long transcription looked like a
      // frozen click. It closes in the finally block (navigation unmounts it).
      setPostRecordingProgress(null);
      try {
        const { meetingId } = pending;
        const folderPath = sessionStorage.getItem('last_recording_folder_path');

        if (action !== 'keep' && actionNeedsTranscription(action) && folderPath) {
          try {
            toast.info(tr('toast.transcribing'), {
              description: tr('toast.transcribingHint'),
            });

            await invoke('start_retranscription_command', {
              meetingId,
              meetingFolderPath: folderPath,
              ...(await resolveBatchTarget()),
            });

            await waitForRetranscription(meetingId, setPostRecordingProgress);
            toast.success(tr('toast.transcriptionComplete'));
          } catch (error) {
            console.error('Post-recording follow-up failed:', error);
            toast.error(tr('toast.transcriptionFailed'), {
              description: error instanceof Error ? error.message : String(error),
            });
          }
        }

        // The summary itself is generated on the meeting page, which owns the model
        // configuration and the transcript payload. A one-shot request tells that
        // page the user asked for it here, even when auto-summary is switched off.
        if (actionNeedsSummary(action)) {
          sessionStorage.setItem('meetily.requestedSummaryMeeting', meetingId);
        }

        navigateToMeeting(meetingId);
      } finally {
        setPostRecordingProgress(null);
        setPendingPostRecording(null);
      }
    },
    [pendingPostRecording, navigateToMeeting, resolveBatchTarget, waitForRetranscription],
  );

  // Expose handleRecordingStop function to window for Rust callbacks
  const handleRecordingStopRef = useRef(handleRecordingStop);
  useEffect(() => {
    handleRecordingStopRef.current = handleRecordingStop;
  });

  useEffect(() => {
    (window as any).handleRecordingStop = (callApi: boolean = true) => {
      handleRecordingStopRef.current(callApi);
    };

    // Cleanup on unmount
    return () => {
      delete (window as any).handleRecordingStop;
    };
  }, []);

  // Derive summaryStatus from RecordingStatus for backward compatibility
  const summaryStatus: SummaryStatus = status === RecordingStatus.PROCESSING_TRANSCRIPTS ? 'processing' : 'idle';

  return {
    handleRecordingStop,
    isStopping,
    isProcessingTranscript,
    isSavingTranscript,
    summaryStatus,
    setIsStopping: (value: boolean) => {
      setStatus(value ? RecordingStatus.STOPPING : RecordingStatus.IDLE);
    },
    pendingPostRecording,
    handlePostRecordingChoice,
    postRecordingProgress,
  };
}
