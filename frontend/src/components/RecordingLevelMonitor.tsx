'use client';
import { tr } from '@/lib/i18n';

import React, { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { AlertTriangle, Mic, MonitorSpeaker } from 'lucide-react';
import { useConfig } from '@/contexts/ConfigContext';
import {
  LevelTracker,
  formatDbfs,
  toDbfs,
  AUDIBLE_RMS,
  CLIPPING_PEAK,
  SILENCE_DB,
  QUIET_DB,
  type LevelDiagnosis,
  type LevelSample,
} from '@/lib/audio-levels';

interface AudioLevelUpdate {
  levels: Array<{
    device_name: string;
    device_type?: string;
    rms_level: number;
    peak_level: number;
  }>;
}

interface RecordingLevelMonitorProps {
  /** Levels are only meaningful while capture is running. */
  active: boolean;
  className?: string;
}

/**
 * Live input-level feedback during a recording.
 *
 * The device-selection screen already meters audio, but once recording starts
 * there was no indication at all: a muted microphone or an output device that is
 * not the one playing the meeting produced a silent track that went unnoticed
 * until the meeting was over. This surfaces that within seconds.
 */
export function RecordingLevelMonitor({ active, className = '' }: RecordingLevelMonitorProps) {
  const { captureMicrophone, captureSystemAudio, selectedDevices, selectedWindow } = useConfig();
  const [diagnosis, setDiagnosis] = useState<LevelDiagnosis | null>(null);
  const [latest, setLatest] = useState<LevelSample[]>([]);
  const trackerRef = useRef(new LevelTracker());
  // Clipping is an event, not a level: the peak can fall immediately after a
  // clipped sample. Latch it briefly so the red state is visible rather than a
  // one-frame flicker.
  const clipHoldRef = useRef<Record<string, number>>({});
  // Levels arrive one device at a time. Keeping only the last event made the
  // second source disappear as soon as the first one reported again, which is why
  // the bar could show the microphone and nothing else even while both were live.
  const [samplesByDevice, setSamplesByDevice] = useState<
    Record<string, LevelSample & { device_type?: string; last_seen: number }>
  >({});

  useEffect(() => {
    if (!active) {
      trackerRef.current.reset();
      setDiagnosis(null);
      setLatest([]);
      setSamplesByDevice({});
      clipHoldRef.current = {};
      return;
    }

    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    listen<AudioLevelUpdate>('audio-levels', (event) => {
      const levels = event.payload?.levels ?? [];
      if (levels.length === 0) return;

      const samples: LevelSample[] = levels.map((level) => ({
        device_name: level.device_name,
        rms_level: level.rms_level,
        peak_level: level.peak_level,
      }));

      trackerRef.current.push(samples);
      setLatest(samples);
      // Merge rather than replace: each event carries only the device that just
      // reported, and the other source's last level should stay on screen.
      setSamplesByDevice((previous) => {
        const next = { ...previous };
        levels.forEach((level) => {
          if (level.peak_level >= CLIPPING_PEAK) {
            clipHoldRef.current[level.device_name] = Date.now();
          }
          next[level.device_name] = {
            device_name: level.device_name,
            device_type: level.device_type,
            rms_level: level.rms_level,
            peak_level: level.peak_level,
            last_seen: Date.now(),
          };
        });
        return next;
      });
      setDiagnosis(trackerRef.current.diagnose());
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

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [active]);

  if (!active) return null;

  const deviceSuffix = diagnosis?.deviceName ? ` (${diagnosis.deviceName})` : '';
  const warning = !diagnosis
    ? null
    : diagnosis.verdict === 'silent'
      ? tr('recordingLevel.silent', { device: deviceSuffix })
      : diagnosis.verdict === 'quiet'
        ? tr('recordingLevel.quiet', { device: deviceSuffix })
        : diagnosis.verdict === 'clipping'
          ? tr('recordingLevel.clipping', { device: deviceSuffix })
          : null;

  // A source that stops reporting (muted device, closed window) must not keep its
  // last green sample forever; the other source's events still trigger renders, so
  // staleness is evaluated against the wall clock on each render.
  const now = Date.now();
  const samples = Object.values(samplesByDevice).filter(
    (sample) => now - sample.last_seen <= 2_000,
  );
  const micSample = samples.find((sample) => sample.device_type !== 'system');
  const systemSample = samples.find((sample) => sample.device_type === 'system');
  const visibleSamples = captureMicrophone || captureSystemAudio ? samples : latest;

  const micName = micSample?.device_name
    || selectedDevices?.micDevice
    || tr('devices.defaultMicrophone');
  const systemName = selectedWindow?.title
    ? tr('live.sourceWindow', { name: selectedWindow.title })
    : systemSample?.device_name || selectedDevices?.systemDevice || tr('devices.defaultSystemAudio');

  const statusFor = (sample: LevelSample | undefined) => {
    if (!sample) {
      return { label: tr('live.sourceStatusWaiting'), dot: 'bg-slate-300' };
    }
    if (
      sample.peak_level >= CLIPPING_PEAK
      || now - (clipHoldRef.current[sample.device_name] ?? 0) < 1_500
    ) {
      return { label: tr('live.sourceStatusClipping'), dot: 'bg-red-500' };
    }
    if (sample.rms_level < AUDIBLE_RMS) {
      return { label: tr('live.sourceStatusSilent'), dot: 'bg-amber-500' };
    }
    return { label: tr('live.sourceStatusOk'), dot: 'bg-green-500' };
  };
  const micStatus = statusFor(micSample);
  const systemStatus = statusFor(systemSample);

  return (
    <div className={`space-y-2 ${className}`}>
      {(captureMicrophone || captureSystemAudio) && (
        <div className="flex flex-wrap items-center gap-2 rounded-xl bg-slate-50/80 border border-slate-100 px-2.5 py-2">
          <span className="text-[10px] font-medium uppercase tracking-[0.08em] text-slate-400">{tr('live.sources')}</span>
          {captureMicrophone && (
            <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-slate-200 bg-white px-2 py-1 text-[11px] text-slate-700 shadow-sm">
              <Mic className="h-3 w-3 shrink-0 text-slate-400" />
              <span className="max-w-[11rem] truncate" title={micName}>{micName}</span>
              <span className="inline-flex shrink-0 items-center gap-1 text-[10px] text-slate-500">
                <span className={`h-1.5 w-1.5 rounded-full ${micStatus.dot}`} />
                {micStatus.label}
              </span>
            </span>
          )}
          {captureSystemAudio && (
            <span className="inline-flex min-w-0 items-center gap-1.5 rounded-full border border-slate-200 bg-white px-2 py-1 text-[11px] text-slate-700 shadow-sm">
              <MonitorSpeaker className="h-3 w-3 shrink-0 text-slate-400" />
              <span className="max-w-[11rem] truncate" title={systemName}>{systemName}</span>
              <span className="inline-flex shrink-0 items-center gap-1 text-[10px] text-slate-500">
                <span className={`h-1.5 w-1.5 rounded-full ${systemStatus.dot}`} />
                {systemStatus.label}
              </span>
            </span>
          )}
        </div>
      )}

      <div className="flex items-center gap-2">
        <div className="flex-1 space-y-1">
          {visibleSamples.length === 0 ? (
            <p className="text-xs text-gray-500">{tr('live.waitingAudio')}</p>
          ) : (
            visibleSamples.map((sample) => (
              <LevelBar key={sample.device_name} sample={sample} />
            ))
          )}
          {latest.length > 0 && captureMicrophone && !micSample && (
            <p className="text-[11px] text-amber-700">{tr('live.sourceMicWaiting')}</p>
          )}
          {latest.length > 0 && captureSystemAudio && !systemSample && (
            <p className="text-[11px] text-amber-700">{tr('live.sourceSystemWaiting')}</p>
          )}
        </div>
        {diagnosis && (
          <span className="text-xs font-mono text-gray-500 min-w-[4.5rem] text-right">
            {formatDbfs(diagnosis.meanRmsDb)}
          </span>
        )}
      </div>

      {warning && (
        <div
          className={`mt-1 flex items-start gap-2 rounded-lg border px-2.5 py-2 text-[11px] leading-relaxed ${
            diagnosis?.verdict === 'clipping'
              ? 'bg-red-50 border-red-200 text-red-900'
              : 'bg-amber-50 border-amber-200 text-amber-900'
          }`}
          role="status"
          aria-live="polite"
        >
          <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0" />
          <span>{warning}</span>
        </div>
      )}
    </div>
  );
}

/** Map dBFS onto the meter, with -60 dB as the left edge and 0 dB as full scale. */
function meterPercent(db: number): number {
  if (!Number.isFinite(db)) return 0;
  return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
}

/** A single device's level bar. */
function LevelBar({ sample }: { sample: LevelSample }) {
  // Level meters are read in dBFS, so the bar maps -60..0 dB linearly rather than
  // stretching linear amplitude. That keeps speech, ambience and clipping in the
  // ranges broadcast meters expect instead of bunching everything at the bottom.
  const rmsDb = toDbfs(sample.rms_level);
  const peakDb = toDbfs(sample.peak_level);
  const rmsPercent = meterPercent(rmsDb);
  const peakPercent = meterPercent(peakDb);

  // Green is the healthy speech range; amber means the signal is too quiet to
  // transcribe reliably; red is reserved for clipping, which is damaging.
  const colour =
    sample.peak_level >= CLIPPING_PEAK
      ? 'bg-red-500'
      : rmsDb < SILENCE_DB
        ? 'bg-slate-400'
        : rmsDb < QUIET_DB
          ? 'bg-amber-500'
          : 'bg-green-500';

  const dbLabel = formatDbfs(rmsDb);

  return (
    <div className="flex items-center gap-2 py-0.5">
      <span className="w-28 truncate text-[10px] text-slate-500" title={sample.device_name}>
        {sample.device_name}
      </span>
      <div
        role="meter"
        aria-label={sample.device_name}
        aria-valuemin={-60}
        aria-valuemax={0}
        aria-valuenow={Number.isFinite(rmsDb) ? Math.max(-60, Math.round(rmsDb)) : -60}
        aria-valuetext={dbLabel}
        className="relative h-1.5 flex-1 overflow-hidden rounded-full bg-slate-100 shadow-[inset_0_1px_2px_rgba(15,23,42,0.08)]"
      >
        <div
          className={`h-full rounded-full ${colour} transition-[width,background-color] duration-150 ease-out`}
          style={{ width: `${rmsPercent}%` }}
        />
        {peakPercent > rmsPercent && (
          <div
            className="absolute bottom-0 top-0 w-0.5 rounded-full bg-slate-600/70"
            style={{ left: `${peakPercent}%` }}
          />
        )}
      </div>
      <span className="w-14 text-right font-mono text-[10px] tabular-nums text-slate-500">{dbLabel}</span>
    </div>
  );
}
