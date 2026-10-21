import { describe, expect, test } from 'bun:test';
import {
  compareTranscripts,
  insertTranscript,
  mergeTranscripts,
} from '@/contexts/TranscriptContext';
import type { Transcript } from '@/types';

/** A segment ordered by recording time (chunk_start_time) and arrival order. */
const segment = (sequenceId: number, chunkStartTime = sequenceId * 10): Transcript => ({
  id: `seg-${sequenceId}`,
  text: `text ${sequenceId}`,
  timestamp: '00:00',
  sequence_id: sequenceId,
  chunk_start_time: chunkStartTime,
  is_partial: false,
});

const ids = (list: Transcript[]) => list.map((item) => item.sequence_id);

describe('compareTranscripts', () => {
  test('orders by recording time, then by sequence id', () => {
    expect(compareTranscripts(segment(1, 10), segment(2, 20))).toBeLessThan(0);
    expect(compareTranscripts(segment(2, 20), segment(1, 10))).toBeGreaterThan(0);
    // Same chunk time falls back to arrival order.
    expect(compareTranscripts(segment(1, 10), segment(2, 10))).toBeLessThan(0);
    expect(compareTranscripts(segment(2, 10), segment(1, 10))).toBeGreaterThan(0);
  });
});

describe('mergeTranscripts', () => {
  test('appends an in-order batch without reordering existing segments', () => {
    const existing = [segment(1), segment(2)];
    const merged = mergeTranscripts(existing, [segment(3), segment(4)]);
    expect(ids(merged)).toEqual([1, 2, 3, 4]);
    // Existing items are reused, not copied into new objects.
    expect(merged[0]).toBe(existing[0]);
  });

  test('returns the same array when there is nothing new', () => {
    const existing = [segment(1), segment(2)];
    expect(mergeTranscripts(existing, [])).toBe(existing);
    expect(mergeTranscripts(existing, [segment(1)])).toBe(existing);
  });

  test('drops duplicates by sequence id, including duplicates inside the batch', () => {
    const existing = [segment(1), segment(2)];
    const merged = mergeTranscripts(existing, [segment(2), segment(3), segment(3), segment(4)]);
    expect(ids(merged)).toEqual([1, 2, 3, 4]);
  });

  test('merges an out-of-order batch into the right position', () => {
    const existing = [segment(1), segment(2), segment(5)];
    const merged = mergeTranscripts(existing, [segment(4), segment(3)]);
    expect(ids(merged)).toEqual([1, 2, 3, 4, 5]);
  });

  test('keeps recording-time ordering when chunk times differ from sequence order', () => {
    const existing = [segment(1, 100), segment(2, 200)];
    const merged = mergeTranscripts(existing, [segment(3, 300), segment(4, 150)]);
    expect(ids(merged)).toEqual([1, 4, 2, 3]);
  });

  test('ignores incoming segments without a sequence id (live path requires one)', () => {
    const existing = [segment(1)];
    const noSequence: Transcript = { ...segment(2), sequence_id: undefined };
    expect(mergeTranscripts(existing, [noSequence])).toBe(existing);
  });
});

describe('insertTranscript', () => {
  test('appends in-order segments and inserts early ones in place', () => {
    const existing = [segment(1), segment(3)];
    expect(ids(insertTranscript(existing, segment(4)))).toEqual([1, 3, 4]);
    expect(ids(insertTranscript(existing, segment(2)))).toEqual([1, 2, 3]);
    expect(ids(insertTranscript([], segment(1)))).toEqual([1]);
  });
});
