/**
 * Background finalization of stopped recordings.
 *
 * Stopping a recording only closes the capture. The backend then keeps
 * transcribing the audio that was still queued, saves the recording, and emits
 * `recording-finalized` with the session's complete transcript. The frontend
 * stores the meeting from that payload in the background, so the user can start
 * the next meeting straight away instead of watching "processing recording".
 *
 * While one session finishes, the next one may already be recording, and both
 * emit `transcript-update`. Every update carries its `session_id`; the live
 * transcript view drops updates from sessions that have stopped
 * ({@link isFinishedSession}).
 *
 * Module-level rather than React state: a finalization must survive navigating
 * away from the page that started it, and both stop paths (the main window and
 * the tray / floating bar) share one queue.
 */

import { listen } from '@tauri-apps/api/event';
import type { Transcript } from '@/types';

/** One transcript segment as the backend collected it for a session. */
export interface FinalizedSegment {
  id: string;
  text: string;
  audio_start_time: number;
  audio_end_time: number;
  duration: number;
  display_time: string;
  confidence: number;
  sequence_id: number;
}

/** Payload of `recording-finalized`. */
export interface FinalizedRecording {
  session_id: string;
  folder_path: string | null;
  meeting_name: string | null;
  transcription_deferred: boolean;
  segments: FinalizedSegment[];
  save_error?: string | null;
}

/** Payload of `recording-stopped` (the fields this module reads). */
export interface StoppedRecording {
  session_id?: string | null;
  folder_path?: string | null;
  meeting_name?: string | null;
  transcription_deferred?: boolean;
}

/** Convert backend segments to the shape the meeting save expects, in playback order. */
export function segmentsToTranscripts(segments: FinalizedSegment[]): Transcript[] {
  return [...segments]
    .sort((a, b) => a.audio_start_time - b.audio_start_time || a.sequence_id - b.sequence_id)
    .map((segment) => ({
      id: segment.id,
      text: segment.text,
      timestamp: segment.display_time,
      sequence_id: segment.sequence_id,
      chunk_start_time: segment.audio_start_time,
      is_partial: false,
      confidence: segment.confidence,
      audio_start_time: segment.audio_start_time,
      audio_end_time: segment.audio_end_time,
      duration: segment.duration,
    }));
}

/**
 * Matches `recording-finalized` events to the jobs waiting for them.
 *
 * An event can arrive before its job starts waiting (a short or deferred
 * recording finalizes almost immediately), so unclaimed results are kept.
 */
export function createFinalizedRegistry() {
  const results = new Map<string, FinalizedRecording>();
  const waiters = new Map<string, (result: FinalizedRecording | null) => void>();

  return {
    deliver(result: FinalizedRecording) {
      const waiter = waiters.get(result.session_id);
      if (waiter) {
        waiters.delete(result.session_id);
        waiter(result);
      } else {
        results.set(result.session_id, result);
      }
    },

    /** Resolves with the session's result, or null after `timeoutMs`. */
    wait(sessionId: string, timeoutMs: number): Promise<FinalizedRecording | null> {
      const ready = results.get(sessionId);
      if (ready) {
        results.delete(sessionId);
        return Promise.resolve(ready);
      }
      return new Promise((resolve) => {
        const timer = setTimeout(() => {
          waiters.delete(sessionId);
          resolve(null);
        }, timeoutMs);
        waiters.set(sessionId, (result) => {
          clearTimeout(timer);
          resolve(result);
        });
      });
    },
  };
}

/**
 * Runs jobs one at a time in the order they were added, and counts the ones not
 * yet finished. Saving in stop order keeps meetings listed in recording order,
 * and a failing job never blocks the ones behind it.
 */
export function createSerialQueue() {
  let tail: Promise<void> = Promise.resolve();
  let pending = 0;
  const listeners = new Set<() => void>();
  const notify = () => listeners.forEach((listener) => listener());

  return {
    enqueue(job: () => Promise<void>): Promise<void> {
      pending += 1;
      notify();
      const run = tail.then(job).catch((error) => {
        console.error('[BackgroundFinalize] Job failed:', error);
      }).finally(() => {
        pending -= 1;
        notify();
      });
      tail = run;
      return run;
    },
    pendingCount: () => pending,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/**
 * The backend already bounds the drain (10 min) and the save (5 min); this is
 * only a backstop for a lost event, after which the on-screen transcript is used.
 */
export const FINALIZE_WAIT_MS = 20 * 60 * 1000;

const registry = createFinalizedRegistry();
const queue = createSerialQueue();
const finishedSessions = new Set<string>();
let lastStopped: StoppedRecording | null = null;
const stoppedWaiters: Array<(stopped: StoppedRecording) => void> = [];
let listenersReady: Promise<void> | null = null;

/**
 * Subscribe to the session lifecycle events once per window. Called at app
 * start (from the post-processing provider) so no event is missed.
 */
export function ensureRecordingSessionListeners(): Promise<void> {
  if (!listenersReady) {
    listenersReady = (async () => {
      await listen<StoppedRecording>('recording-stopped', (event) => {
        const payload = event.payload ?? {};
        if (payload.session_id) finishedSessions.add(payload.session_id);
        const waiter = stoppedWaiters.shift();
        if (waiter) waiter(payload);
        else lastStopped = payload;
      });
      await listen<FinalizedRecording>('recording-finalized', (event) => {
        if (event.payload?.session_id) registry.deliver(event.payload);
      });
    })().catch((error) => {
      console.error('[BackgroundFinalize] Failed to subscribe to recording events:', error);
      listenersReady = null;
    });
  }
  return listenersReady;
}

/** True for a session that has stopped; its late transcript updates are not live. */
export function isFinishedSession(sessionId: string | null | undefined): boolean {
  return !!sessionId && finishedSessions.has(sessionId);
}

/**
 * The `recording-stopped` payload for the stop being handled, consumed by the
 * stop handler. The event and the `stop_recording` reply travel separately, so
 * the handler may run first; it then waits briefly for the event.
 */
export function takeStoppedRecording(timeoutMs = 3000): Promise<StoppedRecording | null> {
  if (lastStopped) {
    const stopped = lastStopped;
    lastStopped = null;
    return Promise.resolve(stopped);
  }
  return new Promise((resolve) => {
    const waiter = (stopped: StoppedRecording) => {
      clearTimeout(timer);
      resolve(stopped);
    };
    const timer = setTimeout(() => {
      const index = stoppedWaiters.indexOf(waiter);
      if (index >= 0) stoppedWaiters.splice(index, 1);
      resolve(null);
    }, timeoutMs);
    stoppedWaiters.push(waiter);
  });
}

export const waitForFinalizedRecording = (sessionId: string, timeoutMs = FINALIZE_WAIT_MS) =>
  registry.wait(sessionId, timeoutMs);

export const enqueueFinalization = (job: () => Promise<void>) => queue.enqueue(job);
export const pendingFinalizationCount = () => queue.pendingCount();
export const subscribeFinalizations = (listener: () => void) => queue.subscribe(listener);

/**
 * A finalized recording that skipped live transcription and is waiting for the
 * user to choose what to do with it. Shared so whichever window state shows the
 * dialog sees the same one.
 */
export interface PendingPostRecording {
  meetingId: string;
  folderPath: string | null;
  /** The meeting's name, for background transcription messages. */
  title?: string;
}

let pendingPostRecording: PendingPostRecording | null = null;
const pendingListeners = new Set<() => void>();

export function getPendingPostRecording(): PendingPostRecording | null {
  return pendingPostRecording;
}

export function setPendingPostRecording(value: PendingPostRecording | null) {
  pendingPostRecording = value;
  pendingListeners.forEach((listener) => listener());
}

export function subscribePendingPostRecording(listener: () => void) {
  pendingListeners.add(listener);
  return () => {
    pendingListeners.delete(listener);
  };
}
