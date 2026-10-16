/**
 * Recording level analysis for gain feedback.
 *
 * The app already emits `audio-levels` events and renders a meter on the device
 * selection screen, but during an actual recording there is no feedback at all:
 * a user can record a whole meeting through a muted microphone or an
 * unplugged/wrong output device and only discover it afterwards.
 *
 * This module decides, from the stream of level samples, whether the input is
 * silent, too quiet, clipping, or healthy — and which device is at fault.
 */

/** A single level sample, mirroring the `audio-levels` event payload. */
export interface LevelSample {
  device_name: string;
  rms_level: number;
  peak_level: number;
}

export type LevelVerdict = 'silent' | 'quiet' | 'clipping' | 'healthy';

export interface LevelDiagnosis {
  verdict: LevelVerdict;
  /** Device the verdict refers to (the worst offender when several are silent). */
  deviceName: string | null;
  /** Mean RMS across the window, in dBFS. -Infinity when fully silent. */
  meanRmsDb: number;
  /** Peak across the window. */
  peak: number;
}

/**
 * Thresholds, in dBFS, applied to the *mean* RMS of the window rather than to a
 * single sample so that natural pauses between words do not trigger a warning.
 */
export const SILENCE_DB = -50;
export const QUIET_DB = -40;

/** Peak at or above this counts as clipping. */
export const CLIPPING_PEAK = 0.98;

/** How many samples to keep. Levels arrive ~10×/s, so ~30 samples ≈ 3 s. */
export const WINDOW_SIZE = 30;

/** Samples needed before a verdict is trustworthy. */
export const MIN_SAMPLES_FOR_VERDICT = 15;

/**
 * RMS amplitude at or above which input counts as audible for silence tracking.
 *
 * About -46 dBFS: roughly the boundary between "someone is talking" and "room
 * tone". A quieter threshold would be tripped by electrical noise, making
 * auto-stop-on-silence never fire in a noisy room.
 */
export const AUDIBLE_RMS = 0.005;

/** Convert a linear amplitude to dBFS, matching how audio tools report level. */
export function toDbfs(amplitude: number): number {
  if (!(amplitude > 0)) return -Infinity;
  return 20 * Math.log10(amplitude);
}

/** Format a dBFS value for display. */
export function formatDbfs(db: number): string {
  if (!Number.isFinite(db)) return '−∞ dBFS';
  return `${db.toFixed(0)} dBFS`;
}

/** Human-readable guidance for a verdict. */
export function guidanceFor(verdict: LevelVerdict, deviceName: string | null): string | null {
  const device = deviceName ? ` (${deviceName})` : '';

  switch (verdict) {
    case 'silent':
      return `No audio is reaching${device}. Check the device is not muted and that it is the one actually playing or capturing the meeting.`;
    case 'quiet':
      return `Audio level is very low${device}. Move closer to the microphone or raise the input gain.`;
    case 'clipping':
      return `Audio is clipping${device}, which distorts speech and hurts transcription. Lower the input gain.`;
    case 'healthy':
      return null;
  }
}

/**
 * Sliding-window level tracker.
 *
 * A class rather than a pure function because the verdict depends on history:
 * a single quiet sample is a pause, a consistently quiet window is a problem.
 *
 * It also tracks how long the input has been below the non-silence threshold,
 * which is what an auto-stop-on-silence feature needs. Silence detection is not
 * the same as "recently quiet": a noisy room keeps producing samples that are
 * low but non-zero, so a gap between samples is not evidence of silence. Only a
 * wall-clock gap since the last sample *above* the threshold is.
 */
export class LevelTracker {
  private readonly window: LevelSample[] = [];
  private lastAudibleAt: number | null = null;
  /** Set once any sample has been observed, audible or not. */
  private hasObservedSample = false;
  /** Timestamp of the first observed sample, used when nothing is ever audible. */
  private firstObservedAt: number | null = null;

  constructor(
    private readonly windowSize: number = WINDOW_SIZE,
    /** Amplitude at or above which input counts as audible. */
    private readonly audibleThreshold: number = AUDIBLE_RMS,
  ) {}

  /** Add a batch of samples (one event usually carries one per device). */
  push(samples: LevelSample[], now: number = Date.now()): void {
    for (const sample of samples) {
      this.window.push(sample);
      if (!this.hasObservedSample) {
        this.firstObservedAt = now;
      }
      this.hasObservedSample = true;
      if (sample.rms_level >= this.audibleThreshold) {
        this.lastAudibleAt = now;
      }
    }
    // Drop the oldest entries, keeping the newest `windowSize`.
    if (this.window.length > this.windowSize) {
      this.window.splice(0, this.window.length - this.windowSize);
    }
  }

  reset(): void {
    this.window.length = 0;
    this.lastAudibleAt = null;
    this.hasObservedSample = false;
    this.firstObservedAt = null;
  }

  get sampleCount(): number {
    return this.window.length;
  }

  /**
   * Seconds since audio last exceeded the audible threshold.
   *
   * Returns 0 before any sample has been observed, so a recording cannot
   * auto-stop during startup before the first level event arrives. Once samples
   * are arriving but all are below the threshold, the input genuinely is silent
   * and the elapsed time is measured from the first of them.
   */
  secondsSinceAudible(now: number = Date.now()): number {
    if (!this.hasObservedSample) return 0;
    // `lastAudibleAt` stays null when nothing has ever been audible; measure from
    // the start of observation in that case.
    const reference = this.lastAudibleAt ?? this.firstObservedAt;
    if (reference === null) return 0;
    return Math.max(0, (now - reference) / 1000);
  }

  /**
   * Whether input has been silent for at least `thresholdSeconds`.
   *
   * Requires samples to have been observed at all: a device that never reports
   * is "unknown", not "silent", and must not trigger an auto-stop.
   */
  isSilentFor(thresholdSeconds: number, now: number = Date.now()): boolean {
    if (!this.hasObservedSample) return false;
    return this.secondsSinceAudible(now) >= thresholdSeconds;
  }

  /** Diagnose the current window, or `null` when there is not enough history. */
  diagnose(): LevelDiagnosis | null {
    if (this.window.length < Math.min(MIN_SAMPLES_FOR_VERDICT, this.windowSize)) {
      return null;
    }

    // Assess each device separately: with mic + system audio, one track being
    // silent is exactly the failure worth reporting (e.g. the wrong output
    // device was selected, so the other participants are not being recorded).
    const byDevice = new Map<string, LevelSample[]>();
    for (const sample of this.window) {
      const list = byDevice.get(sample.device_name);
      if (list) {
        list.push(sample);
      } else {
        byDevice.set(sample.device_name, [sample]);
      }
    }

    let worst: LevelDiagnosis | null = null;

    for (const [deviceName, samples] of byDevice) {
      const meanRms =
        samples.reduce((total, sample) => total + sample.rms_level, 0) / samples.length;
      const peak = samples.reduce((max, sample) => Math.max(max, sample.peak_level), 0);
      const meanRmsDb = toDbfs(meanRms);

      let verdict: LevelVerdict;
      if (peak >= CLIPPING_PEAK) {
        verdict = 'clipping';
      } else if (meanRmsDb < SILENCE_DB) {
        verdict = 'silent';
      } else if (meanRmsDb < QUIET_DB) {
        verdict = 'quiet';
      } else {
        verdict = 'healthy';
      }

      const diagnosis: LevelDiagnosis = { verdict, deviceName, meanRmsDb, peak };

      // Clipping is the most urgent, then silence, then quiet.
      if (!worst || severity(diagnosis.verdict) > severity(worst.verdict)) {
        worst = diagnosis;
      }
    }

    return worst;
  }
}

function severity(verdict: LevelVerdict): number {
  switch (verdict) {
    case 'clipping':
      return 3;
    case 'silent':
      return 2;
    case 'quiet':
      return 1;
    case 'healthy':
      return 0;
  }
}
