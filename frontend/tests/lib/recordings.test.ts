import { describe, test, expect } from 'bun:test';
import {
  formatBytes,
  formatDuration,
  formatRecordedAt,
  canTranscribe,
  canRetranscribe,
  canSummarise,
  type RecordingEntry,
} from '../../src/lib/recordings';

/** A minimal entry, overridable per test. */
function entry(overrides: Partial<RecordingEntry> = {}): RecordingEntry {
  return {
    folder_name: 'Team Standup_2026-05-01_10-00',
    folder_path: '/recordings/Team Standup_2026-05-01_10-00',
    meeting_name: 'Team Standup',
    created_at: '2026-05-01T10:00:00Z',
    duration_seconds: 1800,
    audio_file: 'audio.m4a',
    audio_path: '/recordings/Team Standup_2026-05-01_10-00/audio.m4a',
    audio_bytes: 1024,
    has_transcript: false,
    has_summary: false,
    status: 'completed',
    sort_key: 1,
    ...overrides,
  };
}

describe('formatBytes', () => {
  test('reports an unknown size rather than zero', () => {
    // A recording with no audio must not read as "0 B".
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes(NaN)).toBe('—');
  });

  test('scales through the units', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe('3.0 GB');
  });

  test('switches unit exactly at the boundary', () => {
    expect(formatBytes(1023)).toBe('1023 B');
    expect(formatBytes(1024)).toBe('1 KB');
    expect(formatBytes(1024 * 1024)).toBe('1.0 MB');
  });
});

describe('formatDuration', () => {
  test('reports an unknown duration rather than NaN', () => {
    expect(formatDuration(null)).toBe('—');
    expect(formatDuration(undefined)).toBe('—');
    expect(formatDuration(NaN)).toBe('—');
    expect(formatDuration(-1)).toBe('—');
  });

  test('formats minutes and seconds', () => {
    expect(formatDuration(0)).toBe('0:00');
    expect(formatDuration(5)).toBe('0:05');
    expect(formatDuration(65)).toBe('1:05');
    expect(formatDuration(3599)).toBe('59:59');
  });

  test('adds an hours component only when needed', () => {
    expect(formatDuration(3600)).toBe('1:00:00');
    expect(formatDuration(3725)).toBe('1:02:05');
  });
});

describe('formatRecordedAt', () => {
  test('states when the date is unknown', () => {
    expect(formatRecordedAt(null)).toBe('Unknown date');
    expect(formatRecordedAt(undefined)).toBe('Unknown date');
  });

  test('passes an unparseable value through instead of showing Invalid Date', () => {
    expect(formatRecordedAt('not-a-date')).toBe('not-a-date');
  });

  test('renders an ISO timestamp containing the year', () => {
    const formatted = formatRecordedAt('2026-05-01T10:00:00Z');
    expect(formatted).toContain('2026');
    expect(formatted).not.toContain('Invalid');
  });
});

describe('canTranscribe', () => {
  test('is true when audio exists and no transcript yet', () => {
    expect(canTranscribe(entry())).toBe(true);
  });

  test('is false once a transcript exists', () => {
    // Offering it again would discard work the user already has.
    expect(canTranscribe(entry({ has_transcript: true }))).toBe(false);
  });

  test('is false when there is no audio to transcribe', () => {
    expect(canTranscribe(entry({ audio_file: null, audio_path: null }))).toBe(false);
  });

  test('accepts the absolute audio path when the bare file name is missing', () => {
    // The list payload is transitioning to audio_path; either field proves the
    // audio exists, so the Transcribe button must not disappear on one of them.
    const entryWithPathOnly = entry({ audio_file: null, audio_path: '/recordings/x/audio.m4a' });
    expect(canTranscribe(entryWithPathOnly)).toBe(true);
  });

  test('legacy payloads without audio_path still transcribe', () => {
    const legacy = entry({ audio_path: undefined });
    expect(canTranscribe(legacy)).toBe(true);
  });
});

describe('canRetranscribe', () => {
  test('is true whenever audio exists, transcript or not', () => {
    expect(canRetranscribe(entry({ has_transcript: true }))).toBe(true);
    expect(canRetranscribe(entry({ has_transcript: true, has_summary: true }))).toBe(true);
    expect(canRetranscribe(entry({ has_transcript: false }))).toBe(true);
  });

  test('is false when there is nothing to transcribe', () => {
    expect(canRetranscribe(entry({ audio_file: null, audio_path: null }))).toBe(false);
  });

  test('accepts the absolute audio path as proof of audio', () => {
    expect(canRetranscribe(entry({ audio_file: null, audio_path: '/recordings/x/audio.m4a' }))).toBe(true);
  });
});

describe('canSummarise', () => {
  test('is true only when a transcript exists', () => {
    // The summary is written from the transcript.
    expect(canSummarise(entry({ has_transcript: true }))).toBe(true);
    expect(canSummarise(entry({ has_transcript: false }))).toBe(false);
  });

  test('allows re-summarising an existing summary', () => {
    // Regenerating is a normal action, so an existing summary must not disable it.
    expect(canSummarise(entry({ has_transcript: true, has_summary: true }))).toBe(true);
  });
});
