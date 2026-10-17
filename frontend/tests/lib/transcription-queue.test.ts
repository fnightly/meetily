import { describe, expect, test } from 'bun:test';
import {
  initialTranscriptionQueue,
  isTranscriptionPending,
  nextQueuedFolder,
  transcriptionItem,
  transcriptionQueueReducer,
  transcriptionStageKey,
  type TranscriptionQueueEvent,
  type TranscriptionQueueState,
} from '../../src/lib/transcription-queue';

/**
 * The library's transcription queue is sequential because the backend allows
 * only one retranscription at a time. These checks pin the ordering rules
 * (one active item, settle-before-next) without Tauri or a rendered row.
 */

const run = (state: TranscriptionQueueState, ...events: TranscriptionQueueEvent[]): TranscriptionQueueState =>
  events.reduce(transcriptionQueueReducer, state);

const queueOf = (...folders: string[]): TranscriptionQueueState =>
  folders.reduce(
    (state, folderPath) => transcriptionQueueReducer(state, { type: 'enqueue', folderPath }),
    initialTranscriptionQueue,
  );

describe('enqueue', () => {
  test('adds unseen folders behind the existing queue', () => {
    const state = queueOf('a', 'b');

    expect(state.order).toEqual(['a', 'b']);
    expect(state.active).toBeNull();
    expect(state.items.a.status).toBe('queued');
    expect(state.items.b.status).toBe('queued');
    expect(transcriptionItem(state, 'a')).toMatchObject({ folderPath: 'a', percentage: 0 });
  });

  test('ignores a folder that is already queued or running', () => {
    const before = run(queueOf('a'), { type: 'activate', folderPath: 'a' });
    const after = run(before, { type: 'enqueue', folderPath: 'a' });

    expect(after).toBe(before);
  });

  test('a settled folder can be retried and moves to the back of the line', () => {
    const failed = run(queueOf('a', 'b'), { type: 'activate', folderPath: 'a' }, { type: 'fail', folderPath: 'a', message: 'boom' });
    const retried = run(failed, { type: 'enqueue', folderPath: 'a' });

    expect(retried.order).toEqual(['b', 'a']);
    expect(retried.items.a.status).toBe('queued');
    expect(retried.items.a.errorMessage).toBeUndefined();
  });

  test('a known meeting id survives a retry', () => {
    const withId = run(queueOf('a'), { type: 'resolved', folderPath: 'a', meetingId: 'm-1' });
    const failed = run(withId, { type: 'activate', folderPath: 'a' }, { type: 'fail', folderPath: 'a', message: 'boom' });
    const retried = run(failed, { type: 'enqueue', folderPath: 'a' });

    expect(retried.items.a.meetingId).toBe('m-1');
  });
});

describe('single active item', () => {
  test('nextQueuedFolder is the head only while the queue is idle', () => {
    expect(nextQueuedFolder(queueOf('a', 'b'))).toBe('a');
    const active = run(queueOf('a', 'b'), { type: 'activate', folderPath: 'a' });
    expect(active.active).toBe('a');
    expect(nextQueuedFolder(active)).toBeUndefined();
  });

  test('activate only when idle and queued', () => {
    const state = queueOf('a');
    const activated = run(state, { type: 'activate', folderPath: 'a' });
    expect(activated.active).toBe('a');

    // Already busy: a second activate is a no-op.
    expect(run(activated, { type: 'activate', folderPath: 'b' })).toBe(activated);
    // Unknown folders are ignored.
    expect(run(state, { type: 'activate', folderPath: 'missing' })).toBe(state);
  });

  test('started moves the active item to running without touching the queue', () => {
    const state = run(queueOf('a', 'b'), { type: 'activate', folderPath: 'a' }, { type: 'started', folderPath: 'a' });

    expect(state.items.a.status).toBe('running');
    expect(state.active).toBe('a');
    expect(state.order).toEqual(['a', 'b']);
  });

  test('a progress event can land before started and still marks the item running', () => {
    const state = run(
      queueOf('a'),
      { type: 'activate', folderPath: 'a' },
      { type: 'progress', folderPath: 'a', stage: 'decoding', percentage: 12 },
      { type: 'started', folderPath: 'a' },
    );

    expect(state.items.a.status).toBe('running');
    expect(state.items.a.stage).toBe('decoding');
    expect(state.items.a.percentage).toBe(12);
  });
});

describe('progress', () => {
  test('records stage and percentage', () => {
    const state = run(
      queueOf('a'),
      { type: 'activate', folderPath: 'a' },
      { type: 'progress', folderPath: 'a', stage: 'transcribing', percentage: 40 },
    );

    expect(state.items.a).toMatchObject({ status: 'running', stage: 'transcribing', percentage: 40 });
  });

  test('clamps an out-of-range percentage instead of breaking the bar', () => {
    const base = run(
      queueOf('a'),
      { type: 'activate', folderPath: 'a' },
      { type: 'progress', folderPath: 'a', stage: 'transcribing', percentage: 10 },
    );

    expect(run(base, { type: 'progress', folderPath: 'a', stage: 'transcribing', percentage: 150 }).items.a.percentage).toBe(100);
    expect(run(base, { type: 'progress', folderPath: 'a', stage: 'transcribing', percentage: -3 }).items.a.percentage).toBe(0);
    expect(run(base, { type: 'progress', folderPath: 'a', stage: 'transcribing', percentage: Number.NaN }).items.a.percentage).toBe(0);
  });

  test('a settled item ignores late progress', () => {
    const done = run(
      queueOf('a'),
      { type: 'activate', folderPath: 'a' },
      { type: 'complete', folderPath: 'a', segmentsCount: 3 },
    );
    expect(run(done, { type: 'progress', folderPath: 'a', stage: 'saving', percentage: 90 })).toBe(done);
  });
});

describe('completion advances the queue', () => {
  test('complete marks done, records the segment count and clears the active slot', () => {
    const state = run(
      queueOf('a', 'b'),
      { type: 'activate', folderPath: 'a' },
      { type: 'started', folderPath: 'a' },
      { type: 'complete', folderPath: 'a', segmentsCount: 42 },
    );

    expect(state.items.a.status).toBe('done');
    expect(state.items.a.segmentsCount).toBe(42);
    expect(state.items.a.percentage).toBe(100);
    expect(state.active).toBeNull();
    expect(state.order).toEqual(['b']);
    expect(nextQueuedFolder(state)).toBe('b');
  });

  test('error keeps the message and advances', () => {
    const state = run(
      queueOf('a', 'b'),
      { type: 'activate', folderPath: 'a' },
      { type: 'started', folderPath: 'a' },
      { type: 'fail', folderPath: 'a', message: 'decoder failed' },
    );

    expect(state.items.a.status).toBe('error');
    expect(state.items.a.errorMessage).toBe('decoder failed');
    expect(state.active).toBeNull();
    expect(nextQueuedFolder(state)).toBe('b');
  });

  test('cancel removes the item and advances', () => {
    const state = run(
      queueOf('a', 'b'),
      { type: 'activate', folderPath: 'a' },
      { type: 'started', folderPath: 'a' },
      { type: 'cancelled', folderPath: 'a' },
    );

    expect(state.items.a.status).toBe('cancelled');
    expect(state.order).toEqual(['b']);
    expect(nextQueuedFolder(state)).toBe('b');
  });

  test('the queue runs one item at a time from head to tail', () => {
    let state = queueOf('a', 'b', 'c');
    const order: string[] = [];
    while (nextQueuedFolder(state)) {
      const folder = nextQueuedFolder(state)!;
      order.push(folder);
      state = run(
        state,
        { type: 'activate', folderPath: folder },
        { type: 'started', folderPath: folder },
        { type: 'complete', folderPath: folder, segmentsCount: 1 },
      );
    }

    expect(order).toEqual(['a', 'b', 'c']);
    expect(state.active).toBeNull();
    expect(state.order).toEqual([]);
  });

  test('completion for an unknown folder changes nothing', () => {
    const state = queueOf('a');
    expect(run(state, { type: 'complete', folderPath: 'missing', segmentsCount: 1 })).toBe(state);
  });
});

describe('progress stage labels', () => {
  test('maps known backend stages to i18n keys', () => {
    expect(transcriptionStageKey('decoding')).toBe('library.stage.decoding');
    expect(transcriptionStageKey('vad')).toBe('library.stage.vad');
    expect(transcriptionStageKey('transcribing')).toBe('library.stage.transcribing');
    expect(transcriptionStageKey('saving')).toBe('library.stage.saving');
    expect(transcriptionStageKey('complete')).toBe('library.stage.complete');
  });

  test('an unknown or empty stage has no translation', () => {
    // The caller falls back to the backend text rather than hiding it.
    expect(transcriptionStageKey('new-stage')).toBeNull();
    expect(transcriptionStageKey('')).toBeNull();
  });
});

describe('busy handling', () => {
  test('waiting keeps the item queued and flagged, blocking the next item', () => {
    const state = run(
      queueOf('a', 'b'),
      { type: 'activate', folderPath: 'a' },
      { type: 'waiting', folderPath: 'a' },
    );

    expect(state.items.a.status).toBe('queued');
    expect(state.items.a.waitingForBusy).toBe(true);
    expect(state.active).toBe('a');
    expect(nextQueuedFolder(state)).toBeUndefined();
    expect(isTranscriptionPending(state, 'a')).toBe(true);
  });

  test('resolved stores the materialised meeting id', () => {
    const state = run(queueOf('a'), { type: 'resolved', folderPath: 'a', meetingId: 'meeting-9' });
    expect(state.items.a.meetingId).toBe('meeting-9');
  });
});
