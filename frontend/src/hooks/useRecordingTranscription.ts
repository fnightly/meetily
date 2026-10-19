'use client';

import { useCallback, useEffect, useRef } from 'react';
import type { RecordingEntry } from '@/lib/recordings';
import type { ModelOption } from '@/lib/transcription-model-options';
import type { TranscriptionQueueItem } from '@/lib/transcription-queue';
import {
  cancelTranscription,
  enqueueTranscription,
  onBackgroundJobSettled,
} from '@/lib/background-transcription';
import { useBackgroundTranscription } from '@/hooks/useBackgroundTranscription';

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

const isPendingStatus = (status: TranscriptionQueueItem['status']) =>
  status === 'queued' || status === 'running';

/**
 * The recordings library's view of the background transcription queue.
 *
 * The queue itself lives in `lib/background-transcription` so a transcription
 * keeps running, and still reports when it finishes, after the library page is
 * left; this hook only adapts it to the library's rows.
 */
export function useRecordingTranscription(
  options: UseRecordingTranscriptionOptions,
): RecordingTranscription {
  const { recordings: state } = useBackgroundTranscription();

  const meetingIdForRef = useRef(options.meetingIdFor);
  const onFinishedRef = useRef(options.onFinished);
  useEffect(() => {
    meetingIdForRef.current = options.meetingIdFor;
    onFinishedRef.current = options.onFinished;
  });

  useEffect(
    () =>
      onBackgroundJobSettled((job) => {
        if (job.kind !== 'transcribe' || !job.folderPath) return;
        Promise.resolve(onFinishedRef.current?.(job.folderPath)).catch((error) => {
          console.error('Failed to refresh after transcription:', error);
        });
      }),
    [],
  );

  const enqueue = useCallback((entries: RecordingEntry[], model?: Pick<ModelOption, 'provider' | 'name'>) => {
    enqueueTranscription(
      entries.map((entry) => ({
        folderPath: entry.folder_path,
        meetingId: meetingIdForRef.current(entry.folder_path) ?? null,
        title: entry.meeting_name,
        model,
      })),
    );
  }, []);

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
    cancel: cancelTranscription,
    statusFor,
    isPending,
    activeFolder: state.active,
    pendingCount: state.order.length,
  };
}
