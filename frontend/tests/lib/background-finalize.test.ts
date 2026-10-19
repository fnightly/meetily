import { describe, expect, test } from 'bun:test';
import {
  createFinalizedRegistry,
  createSerialQueue,
  segmentsToTranscripts,
  type FinalizedRecording,
  type FinalizedSegment,
} from '../../src/lib/background-finalize';
import { formatClock } from '../../src/lib/recording-bar';

/**
 * Stopping a recording hands the rest of the work to a background queue so the
 * next meeting can start at once. These checks pin the pieces that make that
 * safe: results are matched to their session whatever order they arrive in,
 * finalizations run one at a time in stop order, and one failure does not
 * strand the recordings behind it.
 */

const segment = (sequence_id: number, audio_start_time: number, text = `s${sequence_id}`): FinalizedSegment => ({
  id: `seg_${sequence_id}`,
  text,
  audio_start_time,
  audio_end_time: audio_start_time + 2,
  duration: 2,
  display_time: '10:00:00',
  confidence: 0.9,
  sequence_id,
});

const finalized = (session_id: string): FinalizedRecording => ({
  session_id,
  folder_path: `/recordings/${session_id}`,
  meeting_name: `Meeting ${session_id}`,
  transcription_deferred: false,
  segments: [],
});

describe('segmentsToTranscripts', () => {
  test('orders by audio time, then sequence, and keeps the timings', () => {
    const transcripts = segmentsToTranscripts([segment(3, 10), segment(1, 0), segment(2, 10)]);
    expect(transcripts.map((t) => t.sequence_id)).toEqual([1, 2, 3]);
    expect(transcripts[0]).toMatchObject({
      text: 's1',
      timestamp: '10:00:00',
      audio_start_time: 0,
      audio_end_time: 2,
      chunk_start_time: 0,
      is_partial: false,
    });
  });
});

describe('createFinalizedRegistry', () => {
  test('a result delivered before anyone waits is kept for the waiter', async () => {
    const registry = createFinalizedRegistry();
    registry.deliver(finalized('a'));
    expect(await registry.wait('a', 1000)).toMatchObject({ session_id: 'a' });
  });

  test('a waiter only receives its own session', async () => {
    const registry = createFinalizedRegistry();
    const waiting = registry.wait('b', 1000);
    registry.deliver(finalized('a'));
    registry.deliver(finalized('b'));
    expect((await waiting)?.session_id).toBe('b');
    expect((await registry.wait('a', 1000))?.session_id).toBe('a');
  });

  test('resolves null when the result never arrives', async () => {
    const registry = createFinalizedRegistry();
    expect(await registry.wait('lost', 5)).toBeNull();
  });
});

describe('createSerialQueue', () => {
  test('runs jobs one at a time in the order they were added', async () => {
    const queue = createSerialQueue();
    const order: string[] = [];
    let running = 0;
    let maxRunning = 0;
    const job = (name: string, ms: number) => async () => {
      running += 1;
      maxRunning = Math.max(maxRunning, running);
      await new Promise((resolve) => setTimeout(resolve, ms));
      order.push(name);
      running -= 1;
    };

    const first = queue.enqueue(job('first', 20));
    const second = queue.enqueue(job('second', 0));
    expect(queue.pendingCount()).toBe(2);
    await Promise.all([first, second]);

    expect(order).toEqual(['first', 'second']);
    expect(maxRunning).toBe(1);
    expect(queue.pendingCount()).toBe(0);
  });

  test('a failing job does not block the next one', async () => {
    const queue = createSerialQueue();
    let ranAfterFailure = false;
    void queue.enqueue(async () => {
      throw new Error('save failed');
    });
    await queue.enqueue(async () => {
      ranAfterFailure = true;
    });
    expect(ranAfterFailure).toBe(true);
  });

  test('notifies subscribers as the pending count changes', async () => {
    const queue = createSerialQueue();
    const seen: number[] = [];
    const unsubscribe = queue.subscribe(() => seen.push(queue.pendingCount()));
    await queue.enqueue(async () => {});
    unsubscribe();
    expect(seen).toEqual([1, 0]);
  });
});

describe('formatClock', () => {
  test('shows minutes and seconds, adding hours from one hour', () => {
    expect(formatClock(0)).toBe('0:00');
    expect(formatClock(65.9)).toBe('1:05');
    expect(formatClock(3600 + 2 * 60 + 3)).toBe('1:02:03');
    expect(formatClock(-4)).toBe('0:00');
  });
});
