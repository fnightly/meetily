'use client';

import { useSyncExternalStore } from 'react';
import {
  getBackgroundTranscriptionSnapshot,
  subscribeBackgroundTranscription,
  type BackgroundTranscriptionSnapshot,
} from '@/lib/background-transcription';

/** The background transcription queues, re-rendering when they change. */
export function useBackgroundTranscription(): BackgroundTranscriptionSnapshot {
  return useSyncExternalStore(
    subscribeBackgroundTranscription,
    getBackgroundTranscriptionSnapshot,
    getBackgroundTranscriptionSnapshot,
  );
}
