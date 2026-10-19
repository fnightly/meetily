'use client';

import { useCallback, useEffect, useReducer, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { tr } from '@/lib/i18n';
import type { RecordingEntry } from '@/lib/recordings';
import {
  batchTranscriptionTarget,
  type ActiveSpeechSettings,
} from '@/lib/transcription-target';
import { batchTargetForModel, type ModelOption } from '@/lib/transcription-model-options';
import {
  initialTranscriptionQueue,
  nextQueuedFolder,
  transcriptionQueueReducer,
  type TranscriptionQueueItem,
  type TranscriptionQueueState,
} from '@/lib/transcription-queue';

/** The backend's single-job guard rejects a second start with this wording. */
const BUSY_ERROR = 'already in progress';
const BUSY_POLL_MS = 1_000;
const BUSY_WAIT_TIMEOUT_MS = 30 * 60 * 1_000;

interface RetranscriptionProgressPayload {
  meeting_id: string;
  stage: string;
  progress_percentage: number;
  message: string;
}

interface RetranscriptionCompletePayload {
  meeting_id: string;
  segments_count: number;
  duration_seconds: number;
  language: string | null;
}

interface RetranscriptionErrorPayload {
  meeting_id: string;
  error: string;
}

/** Marker for a cancel the user asked for, so it is not reported as a failure. */
class TranscriptionCancelled extends Error {}

export interface UseRecordingTranscriptionOptions {
  /** The library's current rows, used for titles and known meeting ids. */
  recordings: RecordingEntry[];
  /** Folder path -> meeting id from the merged sidebar list. */
  meetingIdFor: (folderPath: string) => string | undefined;
  /** Called after an item settles so the library can reload recordings and badges. */
  onFinished?: (folderPath: string) => void | Promise<void>;
}

export interface RecordingTranscription {
  /**
   * Queue recordings; duplicates that are already queued/running are ignored.
   * `model` overrides the speech settings for these recordings only; a retry
   * without one reuses the model the recording was last queued with.
   */
  enqueue: (entries: RecordingEntry[], model?: Pick<ModelOption, 'provider' | 'name'>) => void;
  /** Cancel one queued or running recording. */
  cancel: (folderPath: string) => void;
  statusFor: (folderPath: string) => TranscriptionQueueItem | undefined;
  isPending: (folderPath: string) => boolean;
  /** Folder currently being started or transcribed. */
  activeFolder: string | null;
  /** Items still waiting or running. */
  pendingCount: number;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const isPendingStatus = (status: TranscriptionQueueItem['status']) =>
  status === 'queued' || status === 'running';

/**
 * Sequential transcription queue for the recordings library.
 *
 * Only one retranscription can run at a time (a backend guard), so this hook
 * keeps a FIFO of folders and starts the next one only after the current one
 * reports complete/error/cancelled. The pure ordering rules live in
 * `lib/transcription-queue`; this file is the Tauri/React shell around them.
 */
export function useRecordingTranscription(
  options: UseRecordingTranscriptionOptions,
): RecordingTranscription {
  const [state, dispatch] = useReducer(transcriptionQueueReducer, initialTranscriptionQueue);

  const stateRef = useRef<TranscriptionQueueState>(state);
  const recordingsRef = useRef(options.recordings);
  const meetingIdForRef = useRef(options.meetingIdFor);
  const onFinishedRef = useRef(options.onFinished);
  const cancelledRef = useRef<Set<string>>(new Set());
  /** meeting id -> folder, maintained synchronously so an early event still matches. */
  const meetingToFolderRef = useRef<Map<string, string>>(new Map());
  /** Folder -> model chosen for that recording, when not the configured one. */
  const modelForRef = useRef<Map<string, Pick<ModelOption, 'provider' | 'name'>>>(new Map());
  /** Guards against a StrictMode double effect starting the same item twice. */
  const inFlightRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  useEffect(() => {
    recordingsRef.current = options.recordings;
    meetingIdForRef.current = options.meetingIdFor;
    onFinishedRef.current = options.onFinished;
  });

  const nameFor = useCallback((folderPath: string): string => {
    return recordingsRef.current.find((entry) => entry.folder_path === folderPath)?.meeting_name ?? folderPath;
  }, []);

  const notifyFinished = useCallback(async (folderPath: string) => {
    try {
      await onFinishedRef.current?.(folderPath);
    } catch (error) {
      console.error('Failed to refresh after transcription:', error);
    }
  }, []);

  // The batch command defaults to Whisper when it is not told which engine to use,
  // so a Parakeet or cloud setup must be forwarded explicitly or the recording is
  // transcribed with a Whisper model the user never selected.
  const resolveBatchTarget = useCallback(async () => {
    try {
      const settings = await invoke<ActiveSpeechSettings>('api_get_active_speech_settings');
      return batchTranscriptionTarget(settings);
    } catch (error) {
      console.error('Failed to read the active speech settings:', error);
      return batchTranscriptionTarget(null);
    }
  }, []);

  const waitForIdle = useCallback(async (folderPath: string) => {
    const deadline = Date.now() + BUSY_WAIT_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (cancelledRef.current.has(folderPath)) return;
      const busy = await invoke<boolean>('is_retranscription_in_progress_command');
      if (!busy) return;
      await new Promise((resolve) => setTimeout(resolve, BUSY_POLL_MS));
    }
    throw new Error('Timed out waiting for the running transcription');
  }, []);

  const runItem = useCallback(
    async (folderPath: string) => {
      if (inFlightRef.current.has(folderPath)) return;
      const item = stateRef.current.items[folderPath];
      if (!item) return;

      inFlightRef.current.add(folderPath);
      dispatch({ type: 'activate', folderPath });
      try {
        let meetingId = item.meetingId ?? meetingIdForRef.current(folderPath) ?? null;
        if (!meetingId) {
          // A deferred-recording folder may not have a meeting row yet; the
          // transcript needs one, so materialise it before starting.
          meetingId = await invoke<string>('ensure_meeting_for_recording', {
            folderPath,
            meetingTitle: nameFor(folderPath),
          });
          if (!meetingId) throw new Error('No meeting id was returned');
          meetingToFolderRef.current.set(meetingId, folderPath);
          dispatch({ type: 'resolved', folderPath, meetingId });
        } else {
          meetingToFolderRef.current.set(meetingId, folderPath);
        }

        if (cancelledRef.current.has(folderPath)) throw new TranscriptionCancelled();

        const base = await resolveBatchTarget();
        const chosen = modelForRef.current.get(folderPath);
        const args = {
          meetingId,
          meetingFolderPath: folderPath,
          ...(chosen ? batchTargetForModel(base, chosen) : base),
        };
        try {
          await invoke('start_retranscription_command', args);
        } catch (error) {
          if (!errorMessage(error).toLowerCase().includes(BUSY_ERROR)) throw error;
          // Another job owns the global slot. Stay queued (the row says so),
          // poll until it finishes, then retry the start once.
          dispatch({ type: 'waiting', folderPath });
          await waitForIdle(folderPath);
          if (cancelledRef.current.has(folderPath)) throw new TranscriptionCancelled();
          await invoke('start_retranscription_command', args);
        }

        if (cancelledRef.current.has(folderPath)) {
          // The backend accepted the job, so it must be stopped before the queue
          // may move on: the next start would otherwise hit the busy guard.
          await invoke('cancel_retranscription_command').catch(() => undefined);
          throw new TranscriptionCancelled();
        }

        dispatch({ type: 'started', folderPath });
      } catch (error) {
        const cancelled =
          error instanceof TranscriptionCancelled || cancelledRef.current.has(folderPath);
        if (cancelled) {
          dispatch({ type: 'cancelled', folderPath });
          toast.info(tr('library.transcribeCancelled'));
        } else {
          const message = errorMessage(error);
          dispatch({ type: 'fail', folderPath, message });
          toast.error(tr('library.transcribeFailed'), { description: message });
        }
        await notifyFinished(folderPath);
      } finally {
        inFlightRef.current.delete(folderPath);
        cancelledRef.current.delete(folderPath);
      }
    },
    [nameFor, notifyFinished, resolveBatchTarget, waitForIdle],
  );

  // Start the head of the queue whenever the queue is idle.
  const nextFolder = nextQueuedFolder(state);
  useEffect(() => {
    if (!nextFolder) return;
    void runItem(nextFolder);
  }, [nextFolder, runItem]);

  const enqueue = useCallback((entries: RecordingEntry[], model?: Pick<ModelOption, 'provider' | 'name'>) => {
    for (const entry of entries) {
      const item = stateRef.current.items[entry.folder_path];
      if (item && isPendingStatus(item.status)) continue;
      // Without a model (a retry), the recording keeps the one it was queued with.
      if (model) modelForRef.current.set(entry.folder_path, model);
      dispatch({
        type: 'enqueue',
        folderPath: entry.folder_path,
        meetingId: meetingIdForRef.current(entry.folder_path) ?? null,
      });
    }
  }, []);

  const cancel = useCallback((folderPath: string) => {
    const current = stateRef.current;
    const item = current.items[folderPath];
    if (!item || !isPendingStatus(item.status)) return;

    if (item.status === 'queued' && current.active !== folderPath) {
      // Still in line and not started: nothing to stop at the backend.
      dispatch({ type: 'cancelled', folderPath });
      return;
    }

    cancelledRef.current.add(folderPath);
    if (item.status === 'running') {
      void invoke('cancel_retranscription_command').catch((error) => {
        // The run may have settled between the click and the command; the
        // completion/error event is what actually moves the queue on.
        console.error('Failed to cancel transcription:', error);
      });
    }
    // A queued item that is currently active is being started: running item
    // logic notices the flag after the start resolves and cancels it there.
  }, []);

  // Listen once for the three terminal/progress events. Items are matched by
  // meeting id; events for transcriptions started elsewhere are ignored.
  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const register = (unlisten: UnlistenFn) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };

    const itemForMeeting = (meetingId: string | undefined): TranscriptionQueueItem | undefined => {
      if (!meetingId) return undefined;
      const folder = meetingToFolderRef.current.get(meetingId);
      if (!folder) return undefined;
      const item = stateRef.current.items[folder];
      if (!item || !isPendingStatus(item.status)) return undefined;
      return item;
    };

    const setup = async () => {
      try {
        register(
          await listen<RetranscriptionProgressPayload>('retranscription-progress', (event) => {
            const item = itemForMeeting(event.payload?.meeting_id);
            if (!item) return;
            dispatch({
              type: 'progress',
              folderPath: item.folderPath,
              stage: event.payload.stage ?? '',
              percentage: event.payload.progress_percentage ?? 0,
            });
          }),
        );

        register(
          await listen<RetranscriptionCompletePayload>('retranscription-complete', (event) => {
            const item = itemForMeeting(event.payload?.meeting_id);
            if (!item) return;
            cancelledRef.current.delete(item.folderPath);
            const count = event.payload.segments_count ?? 0;
            dispatch({ type: 'complete', folderPath: item.folderPath, segmentsCount: count });
            toast.success(tr('library.transcribeDone'), {
              description: tr('library.transcribeDoneDetail', {
                name: nameFor(item.folderPath),
                count,
              }),
            });
            void notifyFinished(item.folderPath);
          }),
        );

        register(
          await listen<RetranscriptionErrorPayload>('retranscription-error', (event) => {
            const item = itemForMeeting(event.payload?.meeting_id);
            if (!item) return;
            const message = event.payload.error ?? '';
            const wasCancelled =
              cancelledRef.current.has(item.folderPath) || message.toLowerCase().includes('cancelled');
            cancelledRef.current.delete(item.folderPath);

            if (wasCancelled) {
              dispatch({ type: 'cancelled', folderPath: item.folderPath });
              toast.info(tr('library.transcribeCancelled'));
            } else {
              dispatch({ type: 'fail', folderPath: item.folderPath, message });
              toast.error(tr('library.transcribeFailed'), { description: message });
            }
            void notifyFinished(item.folderPath);
          }),
        );
      } catch (error) {
        console.error('Failed to subscribe to transcription events:', error);
      }
    };

    void setup();

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [nameFor, notifyFinished]);

  const statusFor = useCallback(
    (folderPath: string) => state.items[folderPath],
    [state.items],
  );

  const isPending = useCallback(
    (folderPath: string) => {
      const item = state.items[folderPath];
      return item !== undefined && isPendingStatus(item.status);
    },
    [state.items],
  );

  return {
    enqueue,
    cancel,
    statusFor,
    isPending,
    activeFolder: state.active,
    pendingCount: state.order.length,
  };
}
