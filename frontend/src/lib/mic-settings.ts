/**
 * Pure helpers for the recording-source settings (mic-only capture, gain,
 * noise suppression).
 *
 * Kept free of React/Tauri imports so the clamp/default rules match the Rust
 * backend contract and can be unit tested directly.
 */

/** Accepted range for microphone_gain_db, mirroring the backend validation. */
export const MIN_MICROPHONE_GAIN_DB = -12;
export const MAX_MICROPHONE_GAIN_DB = 24;

/** Unity gain: the recorded microphone signal is left unchanged. */
export const DEFAULT_MICROPHONE_GAIN_DB = 0;

/**
 * Round to whole decibels and clamp into the range the backend accepts.
 *
 * The slider already steps by one; rounding here keeps any other caller (and a
 * corrupt stored value) from producing a fractional gain the backend rejects.
 */
export function clampGainDb(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_MICROPHONE_GAIN_DB;
  return Math.min(MAX_MICROPHONE_GAIN_DB, Math.max(MIN_MICROPHONE_GAIN_DB, Math.round(value)));
}

/**
 * Resolve the persisted gain. Preferences saved by an older backend omit the
 * field; those recordings were made at unity gain.
 */
export function resolveMicrophoneGainDb(value: number | null | undefined): number {
  return typeof value === 'number' ? clampGainDb(value) : DEFAULT_MICROPHONE_GAIN_DB;
}

/**
 * Resolve the persisted capture mode. Older backends omit the field; that build
 * always captured system audio, so a missing value means true.
 */
export function resolveCaptureSystemAudio(value: boolean | null | undefined): boolean {
  return value !== false;
}

/**
 * Resolve whether the microphone is recorded. Older backends omit the field;
 * those builds always captured the microphone, so a missing value means true.
 */
export function resolveCaptureMicrophone(value: boolean | null | undefined): boolean {
  return value !== false;
}

/** Noise suppression is opt-in, so a missing field means off. */
export function resolveNoiseSuppression(value: boolean | null | undefined): boolean {
  return value === true;
}

/** Echo cancellation is opt-in and experimental, so a missing field means off. */
export function resolveEchoCancellation(value: boolean | null | undefined): boolean {
  return value === true;
}

/** Stereo internal recording is on by default; an explicit false disables it. */
export function resolveStereoInternalRecording(value: boolean | null | undefined): boolean {
  return value !== false;
}

/**
 * Format a gain for display with an explicit sign on positive values.
 *
 * The unit comes from the recording.gainValue translation ("{db} dB"), so only
 * the signed number is returned here: formatGainDb(3) -> '+3', which the UI
 * renders as '+3 dB'.
 */
export function formatGainDb(value: number): string {
  const db = clampGainDb(value);
  return db > 0 ? '+' + db : String(db);
}
