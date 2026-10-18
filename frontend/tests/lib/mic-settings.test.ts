import { describe, expect, test } from 'bun:test';
import {
  DEFAULT_MICROPHONE_GAIN_DB,
  MAX_MICROPHONE_GAIN_DB,
  MIN_MICROPHONE_GAIN_DB,
  clampGainDb,
  formatGainDb,
  resolveCaptureMicrophone,
  resolveCaptureSystemAudio,
  resolveEchoCancellation,
  resolveMicrophoneGainDb,
  resolveNoiseSuppression,
} from '@/lib/mic-settings';

describe('microphone gain', () => {
  test('range matches the backend contract', () => {
    expect(MIN_MICROPHONE_GAIN_DB).toBe(-12);
    expect(MAX_MICROPHONE_GAIN_DB).toBe(24);
    expect(DEFAULT_MICROPHONE_GAIN_DB).toBe(0);
  });

  test('clamps out-of-range values into the accepted window', () => {
    expect(clampGainDb(-40)).toBe(-12);
    expect(clampGainDb(-12)).toBe(-12);
    expect(clampGainDb(24)).toBe(24);
    expect(clampGainDb(99)).toBe(24);
    expect(clampGainDb(6)).toBe(6);
  });

  test('rounds fractional values to whole decibels', () => {
    expect(clampGainDb(3.4)).toBe(3);
    expect(clampGainDb(3.6)).toBe(4);
    expect(clampGainDb(-3.6)).toBe(-4);
  });

  test('non-finite input falls back to unity gain', () => {
    expect(clampGainDb(Number.NaN)).toBe(0);
    expect(clampGainDb(Number.POSITIVE_INFINITY)).toBe(0);
  });

  test('missing or invalid stored gain resolves to unity gain', () => {
    expect(resolveMicrophoneGainDb(undefined)).toBe(0);
    expect(resolveMicrophoneGainDb(null)).toBe(0);
    expect(resolveMicrophoneGainDb(Number.NaN)).toBe(0);
    expect(resolveMicrophoneGainDb(6)).toBe(6);
    expect(resolveMicrophoneGainDb(99)).toBe(24);
  });

  test('formats with an explicit sign only on positive values', () => {
    // The UI composes this with the recording.gainValue template ('{db} dB'),
    // so the helper deliberately omits the unit.
    expect(formatGainDb(3)).toBe('+3');
    expect(formatGainDb(24)).toBe('+24');
    expect(formatGainDb(0)).toBe('0');
    expect(formatGainDb(-4)).toBe('-4');
    expect(formatGainDb(-12)).toBe('-12');
    expect(formatGainDb(30)).toBe('+24');
    expect(formatGainDb(-30)).toBe('-12');
  });
});

describe('recording source defaults', () => {
  test('a backend that omits capture_system_audio still captured system audio', () => {
    expect(resolveCaptureSystemAudio(undefined)).toBe(true);
    expect(resolveCaptureSystemAudio(null)).toBe(true);
    expect(resolveCaptureSystemAudio(true)).toBe(true);
    expect(resolveCaptureSystemAudio(false)).toBe(false);
  });

  test('a backend that omits capture_microphone still recorded the microphone', () => {
    expect(resolveCaptureMicrophone(undefined)).toBe(true);
    expect(resolveCaptureMicrophone(null)).toBe(true);
    expect(resolveCaptureMicrophone(true)).toBe(true);
    expect(resolveCaptureMicrophone(false)).toBe(false);
  });

  test('noise suppression is off unless explicitly enabled', () => {
    expect(resolveNoiseSuppression(undefined)).toBe(false);
    expect(resolveNoiseSuppression(null)).toBe(false);
    expect(resolveNoiseSuppression(true)).toBe(true);
    expect(resolveNoiseSuppression(false)).toBe(false);
  });

  test('experimental echo cancellation is off unless explicitly enabled', () => {
    expect(resolveEchoCancellation(undefined)).toBe(false);
    expect(resolveEchoCancellation(null)).toBe(false);
    expect(resolveEchoCancellation(true)).toBe(true);
    expect(resolveEchoCancellation(false)).toBe(false);
  });
});
