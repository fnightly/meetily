import { describe, test, expect } from 'bun:test';
import {
  LevelTracker,
  toDbfs,
  formatDbfs,
  guidanceFor,
  SILENCE_DB,
  QUIET_DB,
  CLIPPING_PEAK,
  WINDOW_SIZE,
  MIN_SAMPLES_FOR_VERDICT,
  type LevelSample,
} from '../../src/lib/audio-levels';

/** Build `count` identical samples for one device. */
function samples(device: string, rms: number, peak: number, count: number): LevelSample[] {
  return Array.from({ length: count }, () => ({
    device_name: device,
    rms_level: rms,
    peak_level: peak,
  }));
}

/** A tracker pre-filled with enough history to produce a verdict. */
function trackerWith(samplesToAdd: LevelSample[]): LevelTracker {
  const tracker = new LevelTracker();
  tracker.push(samplesToAdd);
  return tracker;
}

describe('toDbfs', () => {
  test('maps full scale to 0 dBFS', () => {
    expect(toDbfs(1)).toBeCloseTo(0, 5);
  });

  test('maps half amplitude to about -6 dBFS', () => {
    expect(toDbfs(0.5)).toBeCloseTo(-6.02, 1);
  });

  test('treats zero and negative input as silence rather than NaN', () => {
    expect(toDbfs(0)).toBe(-Infinity);
    expect(toDbfs(-1)).toBe(-Infinity);
  });
});

describe('formatDbfs', () => {
  test('renders finite values with a unit', () => {
    expect(formatDbfs(-20.4)).toBe('-20 dBFS');
  });

  test('renders silence readably instead of "-Infinity"', () => {
    expect(formatDbfs(-Infinity)).toBe('−∞ dBFS');
    expect(formatDbfs(NaN)).toBe('−∞ dBFS');
  });
});

describe('LevelTracker verdicts', () => {
  test('reports nothing until enough history has accumulated', () => {
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.001, 0.002, MIN_SAMPLES_FOR_VERDICT - 1));
    // A short window must not produce a verdict: early samples are not evidence.
    expect(tracker.diagnose()).toBeNull();
  });

  test('classifies a healthy speech level as healthy', () => {
    // Roughly -26 dBFS: a normal speaking level.
    const tracker = trackerWith(samples('Mic', 0.05, 0.3, MIN_SAMPLES_FOR_VERDICT));
    expect(tracker.diagnose()?.verdict).toBe('healthy');
  });

  test('classifies consistent near-silence as silent', () => {
    const tracker = trackerWith(samples('Mic', 0.0001, 0.0005, MIN_SAMPLES_FOR_VERDICT));
    expect(tracker.diagnose()?.verdict).toBe('silent');
  });

  test('classifies a low but non-zero level as quiet', () => {
    // About -46 dBFS: above the silence floor, below the quiet threshold.
    const tracker = trackerWith(samples('Mic', 0.005, 0.02, MIN_SAMPLES_FOR_VERDICT));
    const verdict = tracker.diagnose();
    expect(verdict?.verdict).toBe('quiet');
    expect(verdict!.meanRmsDb).toBeLessThan(QUIET_DB);
    expect(verdict!.meanRmsDb).toBeGreaterThan(SILENCE_DB);
  });

  test('prioritises clipping over a healthy mean level', () => {
    // Loud enough to be healthy, but peaking at full scale.
    const tracker = trackerWith(samples('Mic', 0.2, 1.0, MIN_SAMPLES_FOR_VERDICT));
    expect(tracker.diagnose()?.verdict).toBe('clipping');
  });

  test('treats a peak exactly at the threshold as clipping', () => {
    const tracker = trackerWith(samples('Mic', 0.2, CLIPPING_PEAK, MIN_SAMPLES_FOR_VERDICT));
    expect(tracker.diagnose()?.verdict).toBe('clipping');
  });
});

describe('LevelTracker windowing', () => {
  test('drops the oldest samples once the window is full', () => {
    const tracker = new LevelTracker(10);
    tracker.push(samples('Mic', 0.0001, 0.0001, 10));
    expect(tracker.sampleCount).toBe(10);

    // Feed healthy audio; the old silence must age out entirely.
    tracker.push(samples('Mic', 0.05, 0.3, 10));
    expect(tracker.sampleCount).toBe(10);
    expect(tracker.diagnose()?.verdict).toBe('healthy');
  });

  test('reset clears history so no stale verdict survives', () => {
    const tracker = trackerWith(samples('Mic', 0.0001, 0.0001, MIN_SAMPLES_FOR_VERDICT));
    expect(tracker.diagnose()).not.toBeNull();

    tracker.reset();
    expect(tracker.sampleCount).toBe(0);
    expect(tracker.diagnose()).toBeNull();
  });

  test('default window size is the documented constant', () => {
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.05, 0.3, WINDOW_SIZE + 50));
    expect(tracker.sampleCount).toBe(WINDOW_SIZE);
  });
});

describe('per-device diagnosis', () => {
  test('identifies which device is silent when one track has no audio', () => {
    // The realistic online-meeting failure: the microphone works, but the wrong
    // output device was chosen so the other participants record as silence.
    const tracker = new LevelTracker();
    const mixed: LevelSample[] = [];
    for (let i = 0; i < MIN_SAMPLES_FOR_VERDICT; i++) {
      mixed.push({ device_name: 'Microphone', rms_level: 0.05, peak_level: 0.3 });
      mixed.push({ device_name: 'Speakers', rms_level: 0.0, peak_level: 0.0 });
    }
    tracker.push(mixed);

    const diagnosis = tracker.diagnose();
    expect(diagnosis?.verdict).toBe('silent');
    expect(diagnosis?.deviceName).toBe('Speakers');
  });

  test('reports the whole set as healthy when every track has signal', () => {
    const tracker = new LevelTracker();
    const mixed: LevelSample[] = [];
    for (let i = 0; i < MIN_SAMPLES_FOR_VERDICT; i++) {
      mixed.push({ device_name: 'Microphone', rms_level: 0.05, peak_level: 0.3 });
      mixed.push({ device_name: 'Speakers', rms_level: 0.04, peak_level: 0.25 });
    }
    tracker.push(mixed);

    expect(tracker.diagnose()?.verdict).toBe('healthy');
  });

  test('clipping on any device outranks silence on another', () => {
    const tracker = new LevelTracker();
    const mixed: LevelSample[] = [];
    for (let i = 0; i < MIN_SAMPLES_FOR_VERDICT; i++) {
      mixed.push({ device_name: 'Microphone', rms_level: 0.2, peak_level: 1.0 });
      mixed.push({ device_name: 'Speakers', rms_level: 0.0, peak_level: 0.0 });
    }
    tracker.push(mixed);

    const diagnosis = tracker.diagnose();
    expect(diagnosis?.verdict).toBe('clipping');
    expect(diagnosis?.deviceName).toBe('Microphone');
  });
});

describe('silence duration tracking', () => {
  test('reports no silence before any audio has been observed', () => {
    // A device that never reports is "unknown", not silent, and must not be able
    // to trigger an auto-stop during startup.
    const tracker = new LevelTracker();
    expect(tracker.secondsSinceAudible(1_000_000)).toBe(0);
    expect(tracker.isSilentFor(1, 1_000_000)).toBe(false);
  });

  test('measures time since the last audible sample', () => {
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.05, 0.3, 1), 1000);
    expect(tracker.secondsSinceAudible(4000)).toBeCloseTo(3, 5);
  });

  test('resets the silent duration whenever audio returns', () => {
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.05, 0.3, 1), 1000);
    // Ten seconds pass with nothing audible.
    tracker.push(samples('Mic', 0.0001, 0.0001, 5), 11_000);
    expect(tracker.secondsSinceAudible(11_000)).toBeCloseTo(10, 5);

    // Someone speaks again.
    tracker.push(samples('Mic', 0.06, 0.4, 1), 12_000);
    expect(tracker.secondsSinceAudible(12_000)).toBe(0);
  });

  test('fires only once the threshold is reached', () => {
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.05, 0.3, 1), 0);

    expect(tracker.isSilentFor(30, 29_000)).toBe(false);
    expect(tracker.isSilentFor(30, 30_000)).toBe(true);
    expect(tracker.isSilentFor(30, 60_000)).toBe(true);
  });

  test('a noisy room does not count as silence', () => {
    // Low but continuous room tone: samples keep arriving above the audible
    // threshold, so elapsed time resets even though the level meter reads low.
    const tracker = new LevelTracker();
    for (let second = 0; second <= 60; second++) {
      tracker.push(samples('Mic', 0.01, 0.05, 1), second * 1000);
    }
    expect(tracker.secondsSinceAudible(60_000)).toBe(0);
    expect(tracker.isSilentFor(30, 60_000)).toBe(false);
  });

  test('sub-threshold room tone does count as silence', () => {
    // Below AUDIBLE_RMS the input is treated as nothing happening, which is what
    // lets auto-stop work when background noise is very low.
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.0001, 0.0001, 1), 0);
    expect(tracker.isSilentFor(30, 40_000)).toBe(true);
  });

  test('reset clears silence history along with the window', () => {
    const tracker = new LevelTracker();
    tracker.push(samples('Mic', 0.05, 0.3, 1), 0);
    expect(tracker.isSilentFor(30, 60_000)).toBe(true);

    tracker.reset();
    expect(tracker.secondsSinceAudible(60_000)).toBe(0);
    expect(tracker.isSilentFor(30, 60_000)).toBe(false);
  });

  test('one audible device keeps the recording alive', () => {
    // With mic + system audio, the other participants speaking must reset the
    // timeline even while the microphone itself is silent.
    const tracker = new LevelTracker();
    tracker.push([{ device_name: 'Speakers', rms_level: 0.06, peak_level: 0.4 }], 0);
    tracker.push([{ device_name: 'Microphone', rms_level: 0.0001, peak_level: 0.0001 }], 1000);

    expect(tracker.isSilentFor(30, 20_000)).toBe(false);
    expect(tracker.isSilentFor(30, 31_000)).toBe(true);
  });
});

describe('guidanceFor', () => {
  test('gives no guidance when the level is healthy', () => {
    expect(guidanceFor('healthy', 'Mic')).toBeNull();
  });

  test('names the device in every warning', () => {
    expect(guidanceFor('silent', 'Speakers')).toContain('Speakers');
    expect(guidanceFor('quiet', 'Mic')).toContain('Mic');
    expect(guidanceFor('clipping', 'Mic')).toContain('Mic');
  });

  test('silence guidance points at device selection, not just mute', () => {
    // The common cause is picking the wrong output device, not a muted mic.
    const guidance = guidanceFor('silent', 'Speakers') ?? '';
    expect(guidance.toLowerCase()).toContain('playing');
  });

  test('every verdict produces guidance text except healthy', () => {
    for (const verdict of ['silent', 'quiet', 'clipping'] as const) {
      expect(guidanceFor(verdict, 'Mic')).toBeTruthy();
    }
  });
});
