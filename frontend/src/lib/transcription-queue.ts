/**
 * Pure state and transitions for the recordings library's transcription queue.
 *
 * Kept free of Tauri/React so the ordering rules can be tested directly: only
 * one retranscription runs at a time globally, so starting a second recording
 * means waiting for the first to settle rather than running them in parallel.
 */

/** Where one recording stands in the queue. */
export type TranscriptionStatus = 'queued' | 'running' | 'done' | 'error' | 'cancelled';

export interface TranscriptionQueueItem {
  folderPath: string;
  /** The meeting row transcripts are attached to; resolved lazily when missing. */
  meetingId: string | null;
  status: TranscriptionStatus;
  /** Backend stage from `retranscription-progress` (e.g. "decoding"). */
  stage: string;
  /** 0-100, clamped when a progress event is out of range. */
  percentage: number;
  /** True while this item waits for the single global retranscription slot. */
  waitingForBusy: boolean;
  errorMessage?: string;
  segmentsCount?: number;
}

export interface TranscriptionQueueState {
  /** Folders still to settle, in enqueue order. */
  order: string[];
  /** Every tracked folder, including settled ones, so rows can show the result. */
  items: Record<string, TranscriptionQueueItem>;
  /** The folder being started or transcribed right now; null while idle. */
  active: string | null;
}

export type TranscriptionQueueEvent =
  | { type: 'enqueue'; folderPath: string; meetingId?: string | null }
  | { type: 'resolved'; folderPath: string; meetingId: string }
  | { type: 'activate'; folderPath: string }
  | { type: 'started'; folderPath: string }
  | { type: 'waiting'; folderPath: string }
  | { type: 'progress'; folderPath: string; stage: string; percentage: number }
  | { type: 'complete'; folderPath: string; segmentsCount: number }
  | { type: 'fail'; folderPath: string; message: string }
  | { type: 'cancelled'; folderPath: string };

export const initialTranscriptionQueue: TranscriptionQueueState = {
  order: [],
  items: {},
  active: null,
};

const isPending = (status: TranscriptionStatus) => status === 'queued' || status === 'running';

/** 0-100, integers; a malformed percentage must not blow the progress bar out. */
export function clampPercentage(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, Math.round(value)));
}

function patch(
  state: TranscriptionQueueState,
  folderPath: string,
  changes: Partial<TranscriptionQueueItem>,
): TranscriptionQueueState {
  const item = state.items[folderPath];
  if (!item) return state;
  return { ...state, items: { ...state.items, [folderPath]: { ...item, ...changes } } };
}

function settle(
  state: TranscriptionQueueState,
  folderPath: string,
  item: TranscriptionQueueItem,
): TranscriptionQueueState {
  return {
    order: state.order.filter((folder) => folder !== folderPath),
    items: { ...state.items, [folderPath]: item },
    active: state.active === folderPath ? null : state.active,
  };
}

export function transcriptionQueueReducer(
  state: TranscriptionQueueState,
  event: TranscriptionQueueEvent,
): TranscriptionQueueState {
  switch (event.type) {
    case 'enqueue': {
      const existing = state.items[event.folderPath];
      // Re-clicking a queued row must not start it twice; a settled row can be
      // retried, and keeps the meeting id resolved the first time.
      if (existing && isPending(existing.status)) return state;

      return {
        order: [...state.order.filter((folder) => folder !== event.folderPath), event.folderPath],
        items: {
          ...state.items,
          [event.folderPath]: {
            folderPath: event.folderPath,
            meetingId: event.meetingId ?? existing?.meetingId ?? null,
            status: 'queued',
            stage: '',
            percentage: 0,
            waitingForBusy: false,
          },
        },
        active: state.active,
      };
    }

    case 'resolved':
      return patch(state, event.folderPath, { meetingId: event.meetingId });

    case 'activate': {
      if (state.active !== null) return state;
      const item = state.items[event.folderPath];
      if (!item || item.status !== 'queued') return state;
      return { ...state, active: event.folderPath };
    }

    case 'started': {
      const item = state.items[event.folderPath];
      // A progress event can land before this dispatch; keeping the running
      // state (and its percentage) is better than resetting the bar to zero.
      if (!item || item.status !== 'queued') return state;
      return patch(state, event.folderPath, { status: 'running', waitingForBusy: false, stage: '', percentage: 0 });
    }

    case 'waiting': {
      const item = state.items[event.folderPath];
      if (!item || !isPending(item.status)) return state;
      return patch(state, event.folderPath, { status: 'queued', waitingForBusy: true, stage: '', percentage: 0 });
    }

    case 'progress': {
      const item = state.items[event.folderPath];
      if (!item || !isPending(item.status)) return state;
      return patch(state, event.folderPath, {
        status: 'running',
        waitingForBusy: false,
        stage: event.stage,
        percentage: clampPercentage(event.percentage),
      });
    }

    case 'complete': {
      const item = state.items[event.folderPath];
      if (!item || !isPending(item.status)) return state;
      return settle(state, event.folderPath, {
        ...item,
        status: 'done',
        waitingForBusy: false,
        stage: '',
        percentage: 100,
        segmentsCount: event.segmentsCount,
      });
    }

    case 'fail': {
      const item = state.items[event.folderPath];
      if (!item || !isPending(item.status)) return state;
      return settle(state, event.folderPath, {
        ...item,
        status: 'error',
        waitingForBusy: false,
        stage: '',
        errorMessage: event.message,
      });
    }

    case 'cancelled': {
      const item = state.items[event.folderPath];
      if (!item || !isPending(item.status)) return state;
      return settle(state, event.folderPath, {
        ...item,
        status: 'cancelled',
        waitingForBusy: false,
        stage: '',
        percentage: 0,
      });
    }

    default:
      return state;
  }
}

/** The folder that may be started now: the head of the queue, only while idle. */
export function nextQueuedFolder(state: TranscriptionQueueState): string | undefined {
  if (state.active !== null) return undefined;
  return state.order.find((folder) => state.items[folder]?.status === 'queued');
}

export function transcriptionItem(
  state: TranscriptionQueueState,
  folderPath: string,
): TranscriptionQueueItem | undefined {
  return state.items[folderPath];
}

export function isTranscriptionPending(
  state: TranscriptionQueueState,
  folderPath: string,
): boolean {
  const item = state.items[folderPath];
  return item !== undefined && isPending(item.status);
}

/**
 * i18n key for a backend progress stage, or null for one this build does not know.
 *
 * The backend emits fixed stage ids; mapping them here keeps the interface in the
 * user's language instead of showing the raw id, while an unknown future stage
 * still renders as the backend's own text rather than disappearing.
 */
const TRANSCRIPTION_STAGE_KEYS: Record<string, string> = {
  decoding: 'library.stage.decoding',
  vad: 'library.stage.vad',
  transcribing: 'library.stage.transcribing',
  saving: 'library.stage.saving',
  complete: 'library.stage.complete',
};

export function transcriptionStageKey(stage: string): string | null {
  return TRANSCRIPTION_STAGE_KEYS[stage] ?? null;
}
