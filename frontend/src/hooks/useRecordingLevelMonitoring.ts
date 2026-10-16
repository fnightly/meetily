import { useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { LevelTracker } from '@/lib/audio-levels';

interface AudioLevelUpdate {
  levels: Array<{
    device_name: string;
    rms_level: number;
    peak_level: number;
  }>;
}

export interface RecordingLimits {
  /** Stop after this many seconds of silence. Zero disables. */
  silenceStopSeconds: number;
  /** Hard cap on recording length in seconds. Zero disables. */
  maxRecordingSeconds: number;
}

export interface RecordingLevelMonitoringCallbacks {
  /** Called once when a limit is reached; the reason is user-facing. */
  onLimitReached: (reason: string) => void;
  /** Called when a limit is close, so the UI can warn before it fires. */
  onLimitApproaching?: (reason: string, secondsRemaining: number) => void;
  /** Called when the recordings volume is running low on space. */
  onDiskSpaceLow?: (freeMb: number) => void;
}

/** How long before a limit to warn the user. */
const WARN_BEFORE_SECONDS = 30;

/** How often to evaluate the limits. */
const CHECK_INTERVAL_MS = 1000;

/**
 * How often to re-check free disk space.
 *
 * A recording writes continuously, so a volume that is comfortable at the start
 * can fill up mid-meeting. Checking once a minute is frequent enough to warn
 * while there is still time to act, and cheap enough to ignore.
 */
const DISK_CHECK_INTERVAL_MS = 60_000;

interface DiskSpaceStatus {
  free_mb: number;
  is_low: boolean;
  measured: boolean;
}

/**
 * Start input-level monitoring for the duration of a recording, and enforce the
 * optional recording limits.
 *
 * The limits are evaluated from the level stream rather than inside the audio
 * callback: silence is a property of the mixed signal over time, and stopping a
 * recording must not happen on the audio thread.
 *
 * Monitoring is stopped when recording ends to release the extra capture
 * streams — each monitored device opens its own, and leaving them running keeps
 * the microphone (and its OS indicator) active.
 */
export function useRecordingLevelMonitoring(
  active: boolean,
  deviceNames: string[],
  limits: RecordingLimits,
  callbacks: RecordingLevelMonitoringCallbacks,
): void {
  // Keyed on the joined names so a new array identity each render does not
  // restart monitoring.
  const deviceKey = deviceNames.filter(Boolean).join('\u0000');
  const startedRef = useRef(false);
  // Keep the latest callbacks without restarting the effect that owns the event
  // listener and the interval.
  const callbacksRef = useRef(callbacks);
  callbacksRef.current = callbacks;

  // Limits are read at fire time through a ref for the same reason.
  const limitsRef = useRef(limits);
  limitsRef.current = limits;

  useEffect(() => {
    if (!active || deviceKey.length === 0) {
      return;
    }

    const names = deviceKey.split('\u0000');
    let cancelled = false;

    const tracker = new LevelTracker();
    const startedAt = Date.now();
    let limitFired = false;

    (async () => {
      try {
        await invoke('start_audio_level_monitoring', { deviceNames: names });
        if (!cancelled) {
          startedRef.current = true;
        }
      } catch (error) {
        // Non-fatal: recording itself is unaffected, only the meter and the
        // silence-based limit are unavailable.
        console.error('Failed to start recording level monitoring:', error);
      }
    })();

    let unlisten: UnlistenFn | undefined;
    listen<AudioLevelUpdate>('audio-levels', (event) => {
      const levels = event.payload?.levels ?? [];
      if (levels.length === 0) return;
      tracker.push(
        levels.map((level) => ({
          device_name: level.device_name,
          rms_level: level.rms_level,
          peak_level: level.peak_level,
        })),
      );
    })
      .then((fn) => {
        // The effect may have been cleaned up before `listen` resolved.
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((error) => console.error('Failed to subscribe to audio levels:', error));

    const interval = setInterval(() => {
      if (limitFired || cancelled) return;

      const now = Date.now();
      const elapsedSeconds = (now - startedAt) / 1000;
      const { silenceStopSeconds, maxRecordingSeconds } = limitsRef.current;

      // Length cap first: it is an absolute bound and should win ties.
      if (maxRecordingSeconds > 0) {
        const remaining = maxRecordingSeconds - elapsedSeconds;
        if (remaining <= 0) {
          limitFired = true;
          callbacksRef.current.onLimitReached(
            `Recording stopped at the ${formatDuration(maxRecordingSeconds)} limit.`,
          );
          return;
        }
        if (remaining <= WARN_BEFORE_SECONDS) {
          callbacksRef.current.onLimitApproaching?.(
            'Maximum recording length',
            Math.ceil(remaining),
          );
        }
      }

      if (silenceStopSeconds > 0 && tracker.isSilentFor(silenceStopSeconds, now)) {
        const silentFor = Math.floor(tracker.secondsSinceAudible(now));
        limitFired = true;
        callbacksRef.current.onLimitReached(
          `Recording stopped after ${formatDuration(silentFor)} without audio.`,
        );
        return;
      }

      // Warn shortly before the silence limit would fire, but only once audio has
      // actually been heard — otherwise a slow-starting device would produce a
      // spurious warning.
      if (silenceStopSeconds > 0 && tracker.sampleCount > 0) {
        const silentFor = tracker.secondsSinceAudible(now);
        const remaining = silenceStopSeconds - silentFor;
        if (remaining > 0 && remaining <= WARN_BEFORE_SECONDS && silentFor > 0) {
          callbacksRef.current.onLimitApproaching?.(
            'No audio detected',
            Math.ceil(remaining),
          );
        }
      }
    }, CHECK_INTERVAL_MS);

    // Free-space watch. Checked immediately as well, so a recording started on an
    // already-full disk warns at once rather than a minute later.
    let diskWarned = false;
    const checkDisk = async () => {
      if (cancelled || diskWarned) return;
      try {
        const status = await invoke<DiskSpaceStatus>('get_recording_disk_space');
        // Only warn once: a repeated toast adds noise without new information.
        if (status.measured && status.is_low) {
          diskWarned = true;
          callbacksRef.current.onDiskSpaceLow?.(status.free_mb);
        }
      } catch (error) {
        console.error('Failed to check recording disk space:', error);
      }
    };
    void checkDisk();
    const diskInterval = setInterval(() => void checkDisk(), DISK_CHECK_INTERVAL_MS);

    return () => {
      cancelled = true;
      clearInterval(interval);
      clearInterval(diskInterval);
      unlisten?.();
      if (startedRef.current) {
        startedRef.current = false;
        invoke('stop_audio_level_monitoring').catch((error) =>
          console.error('Failed to stop recording level monitoring:', error),
        );
      }
    };
  }, [active, deviceKey]);
}

/** Render seconds as `m:ss` or `h:mm:ss`. */
export function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;

  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
  }
  return `${minutes}:${String(secs).padStart(2, '0')}`;
}
