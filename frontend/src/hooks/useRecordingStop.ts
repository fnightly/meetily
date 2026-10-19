import { useState, useEffect, useCallback, useRef, useSyncExternalStore } from 'react';
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
import Analytics from '@/lib/analytics';
import {
  actionNeedsSummary,
  actionNeedsTranscription,
  type PostRecordingAction,
} from '@/lib/post-recording';
import { batchTranscriptionTarget, type ActiveSpeechSettings } from '@/lib/transcription-target';
import type { Transcript } from '@/types';
import {
  applyPinnedSummaryLanguageToMeeting,
  detectAndCacheSummaryLanguage,
} from '@/lib/summary-language-preferences';
import {
  enqueueFinalization,
  ensureRecordingSessionListeners,
  getPendingPostRecording,
  segmentsToTranscripts,
  setPendingPostRecording,
  subscribePendingPostRecording,
  takeStoppedRecording,
  waitForFinalizedRecording,
} from '@/lib/background-finalize';

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
 *
 * Stopping returns the UI to idle as soon as the capture is closed, so the next
 * meeting can be recorded right away. The rest runs in the background, one
 * recording at a time (lib/background-finalize): wait for the backend to finish
 * transcribing the queued audio, optional cleanup, SQLite save, analytics, and
 * a toast that links to the meeting.
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
  const pendingPostRecording = useSyncExternalStore(
    subscribePendingPostRecording,
    getPendingPostRecording,
    () => null,
  );
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
    currentMeetingId,
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

  useEffect(() => {
    void ensureRecordingSessionListeners();
  }, []);

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
      Analytics.trackPageView('meeting_details');
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
    [],
  );

  // Main recording stop handler
  const handleRecordingStop = useCallback(async (isCallApi: boolean) => {
    // Guard: prevent duplicate/concurrent stop calls
    if (stopInProgressRef.current) {
      return;
    }
    stopInProgressRef.current = true;

    try {
      await ensureRecordingSessionListeners();

      // Note: stop_recording is already called by RecordingControls.stopRecordingAction
      // (or by the tray / floating bar). By now the capture is closed and the
      // backend is finishing this session in the background.
      // Only a real stop consumes the event: an error path (callApi false) must
      // not take the one belonging to a stop that follows it.
      const stopped = isCallApi ? await takeStoppedRecording() : null;
      const sessionId = stopped?.session_id ?? null;

      // Snapshot what is on screen: the fallback transcript if the backend result
      // never arrives, and the recovery entry that belongs to this recording.
      flushBuffer();
      const onScreenTranscripts = [...transcriptsRef.current];
      const recoveryMeetingId =
        currentMeetingId || sessionStorage.getItem('indexeddb_current_meeting_id');
      const titleAtStop = stopped?.meeting_name || meetingTitle || 'New Meeting';

      // Back to idle now, so the next meeting can start while this one finishes.
      setStatus(RecordingStatus.IDLE);
      setIsRecording(false);
      setIsMeetingActive(false);
      setIsRecordingDisabled(false);

      if (!isCallApi) {
        return;
      }
      if (!stopped) {
        // No stop event: this stop was a duplicate (tray and button at once) or
        // the session never started. The stop that did happen saves the meeting;
        // anything else stays in the recovery store.
        console.warn('No recording-stopped event for this stop; nothing to finalize');
        return;
      }

      console.log('Recording stopped; finishing in the background', {
        session_id: sessionId,
        on_screen_transcripts: onScreenTranscripts.length,
      });

      const toastId = `finalize-${sessionId ?? Date.now()}`;
      toast.loading(tr('toast.finishingRecording', { name: titleAtStop }), {
        id: toastId,
        description: tr('toast.finishingRecordingHint'),
      });

      void enqueueFinalization(() =>
        finalizeRecordingRef.current({
          sessionId,
          toastId,
          titleAtStop,
          onScreenTranscripts,
          recoveryMeetingId,
          folderPathAtStop: stopped.folder_path ?? null,
          deferredAtStop: stopped.transcription_deferred ?? false,
        }),
      );
    } catch (error) {
      console.error('Error in handleRecordingStop:', error);
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : tr('toast.unknownError'));
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
    currentMeetingId,
    meetingTitle,
    setIsMeetingActive,
  ]);

  /**
   * Save one stopped recording once the backend has finished it.
   *
   * Runs from the background queue, possibly while the next meeting records,
   * so it never touches the live recording status or transcript.
   */
  const finalizeRecording = async ({
    sessionId,
    toastId,
    titleAtStop,
    onScreenTranscripts,
    recoveryMeetingId,
    folderPathAtStop,
    deferredAtStop,
  }: {
    sessionId: string | null;
    toastId: string;
    titleAtStop: string;
    onScreenTranscripts: Transcript[];
    recoveryMeetingId: string | null;
    folderPathAtStop: string | null;
    deferredAtStop: boolean;
  }) => {
    try {
      const finalized = sessionId ? await waitForFinalizedRecording(sessionId) : null;
      if (sessionId && !finalized) {
        console.warn('Background finalization result not received; saving the on-screen transcript');
      }

      const deferred = finalized?.transcription_deferred ?? deferredAtStop;
      const folderPath = finalized?.folder_path ?? folderPathAtStop;
      const meetingName = finalized?.meeting_name || titleAtStop;

      // The backend's copy includes the segments transcribed after Stop. Keep the
      // on-screen one only if it somehow holds more.
      let freshTranscripts = finalized ? segmentsToTranscripts(finalized.segments) : [];
      if (freshTranscripts.length < onScreenTranscripts.length) {
        freshTranscripts = onScreenTranscripts;
      }

      // Cleanup runs before saving so the database stores the cleaned text and
      // the user never sees the raw version replaced afterwards.
      freshTranscripts = await applyTranscriptCleanup(freshTranscripts);

      console.log('💾 Saving COMPLETE transcripts to database...', {
        session_id: sessionId,
        transcript_count: freshTranscripts.length,
        meeting_name: meetingName,
        folder_path: folderPath,
      });

      // In deferred mode the meeting is saved even though it has no transcript rows:
      // without a meeting record the audio on disk is unreachable from the UI, so it
      // could never be transcribed afterwards.
      const responseData = await storageService.saveMeeting(meetingName, freshTranscripts, folderPath);
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

      // Mark this recording's recovery entry as saved (not whichever is current now)
      await markMeetingAsSaved(recoveryMeetingId);

      await refetchMeetings();

      // Only move the user if nothing new is being recorded and they are still on
      // the recording page; otherwise the toast's button opens the meeting.
      const recordingNow = await invoke<boolean>('is_recording').catch(() => false);
      const canNavigate = !recordingNow && window.location.pathname === '/';

      if (!recordingNow) {
        try {
          const meetingData = await storageService.getMeeting(meetingId);
          setCurrentMeeting({ id: meetingId, title: meetingData?.title || meetingName });
        } catch (error) {
          console.warn('Could not fetch meeting details, using ID only:', error);
          setCurrentMeeting({ id: meetingId, title: meetingName });
        }
      }

      const openMeeting = {
        label: tr('toast.viewMeeting'),
        onClick: () => {
          router.push(`/meeting-details?id=${meetingId}`);
          Analytics.trackButtonClick('view_meeting_from_toast', 'recording_complete');
        },
      };

      if (deferred) {
        // Deferred transcription has no live text, so the sensible next step is
        // a choice rather than a silent save. Ask, then act on the answer.
        if (canNavigate) {
          toast.success(tr('toast.recordingSaved'), {
            id: toastId,
            description: tr('toast.recordingSavedChoose'),
          });
          setPendingPostRecording({ meetingId, folderPath });
        } else {
          toast.success(tr('toast.recordingSaved'), {
            id: toastId,
            description: tr('toast.recordingSavedLater'),
            action: openMeeting,
            duration: 10000,
          });
        }
      } else {
        toast.success(tr('toast.recordingSavedOk'), {
          id: toastId,
          description: tr('toast.segmentsSaved', { count: freshTranscripts.length }),
          action: openMeeting,
          duration: 10000,
        });

        if (canNavigate) {
          // Auto-navigate after a short delay with source parameter
          setTimeout(async () => {
            const startedMeanwhile = await invoke<boolean>('is_recording').catch(() => false);
            if (!startedMeanwhile && window.location.pathname === '/') {
              navigateToMeeting(meetingId);
            }
          }, 2000);
        }
      }

      if (finalized?.save_error) {
        toast.warning(tr('toast.audioSaveIssue'), { description: finalized.save_error });
      }

      // Track meeting completion analytics
      try {
        // Calculate meeting duration from transcript timestamps
        let durationSeconds = 0;
        if (freshTranscripts.length > 0 && freshTranscripts[0].audio_start_time !== undefined) {
          // Use audio_end_time of last transcript if available
          const lastTranscript = freshTranscripts[freshTranscripts.length - 1];
          durationSeconds = lastTranscript.audio_end_time || lastTranscript.audio_start_time || 0;
        }

        // Calculate word count
        const transcriptWordCount = freshTranscripts
          .map(t => t.text.split(/\s+/).length)
          .reduce((a, b) => a + b, 0);

        // Calculate words per minute
        const wordsPerMinute = durationSeconds > 0 ? transcriptWordCount / (durationSeconds / 60) : 0;

        // Get meetings count today
        const meetingsToday = await Analytics.getMeetingsCountToday();

        // Track meeting completed
        await Analytics.trackMeetingCompleted(meetingId, {
          duration_seconds: durationSeconds,
          transcript_segments: freshTranscripts.length,
          transcript_word_count: transcriptWordCount,
          words_per_minute: wordsPerMinute,
          meetings_today: meetingsToday
        });

        // Update meeting count in analytics.json
        await Analytics.updateMeetingCount();

        // Check for activation (first meeting)
        const { Store } = await import('@tauri-apps/plugin-store');
        const store = await Store.load('analytics.json');
        const totalMeetings = await store.get<number>('total_meetings');

        if (totalMeetings === 1) {
          const daysSinceInstall = await Analytics.calculateDaysSince('first_launch_date');
          await Analytics.track('user_activated', {
            meetings_count: '1',
            days_since_install: daysSinceInstall?.toString() || 'null',
            first_meeting_duration_seconds: durationSeconds.toString()
          });
        }
      } catch (analyticsError) {
        console.error('Failed to track meeting completion analytics:', analyticsError);
        // Don't block user flow on analytics errors
      }
    } catch (saveError) {
      // The transcript stays in the recovery store (not marked saved), so the
      // recovery dialog can still bring it back.
      console.error('Failed to save meeting to database:', saveError);
      toast.error(tr('toast.saveMeetingFailed'), {
        id: toastId,
        description: saveError instanceof Error ? saveError.message : tr('toast.unknownError')
      });
    }
  };
  // The queued job runs later, possibly after this hook re-rendered or unmounted;
  // it uses the latest closure rather than the one from the stop.
  const finalizeRecordingRef = useRef(finalizeRecording);
  finalizeRecordingRef.current = finalizeRecording;

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
        const { meetingId, folderPath } = pending;

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
