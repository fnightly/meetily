'use client';
import { tr } from '@/lib/i18n';

import React, { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { AlertTriangle, Mic, MonitorSpeaker } from 'lucide-react';
import { useConfig } from '@/contexts/ConfigContext';
import {
  LevelTracker,
  formatDbfs,
  AUDIBLE_RMS,
  CLIPPING_PEAK,
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
    if (sample.peak_level >= CLIPPING_PEAK) {
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
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-slate-600">
          <span className="font-medium text-slate-500">{tr('live.sources')}</span>
          {captureMicrophone && (
            <span className="inline-flex items-center gap-1 min-w-0">
              <Mic className="w-3 h-3 shrink-0 text-gray-500" />
              <span className="truncate" title={micName}>{micName}</span>
              <span className="inline-flex items-center gap-1 shrink-0 text-slate-400">
                <span className={`h-1.5 w-1.5 rounded-full ${micStatus.dot}`} />
                {micStatus.label}
              </span>
            </span>
          )}
          {captureSystemAudio && (
            <span className="inline-flex items-center gap-1 min-w-0">
              <MonitorSpeaker className="w-3 h-3 shrink-0 text-gray-500" />
              <span className="truncate" title={systemName}>{systemName}</span>
              <span className="inline-flex items-center gap-1 shrink-0 text-slate-400">
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
          className={`flex items-start gap-2 p-2 rounded border text-xs ${
            diagnosis?.verdict === 'clipping'
              ? 'bg-red-50 border-red-200 text-red-900'
              : 'bg-amber-50 border-amber-200 text-amber-900'
          }`}
          role="status"
        >
          <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0" />
          <span>{warning}</span>
        </div>
      )}
    </div>
  );
}

/** A single device's level bar. */
function LevelBar({ sample }: { sample: LevelSample }) {
  // Log scaling, matching the device-selection meter so the two read the same.
  const scale = (value: number) => {
    const clamped = Math.max(0, Math.min(1, value));
    return clamped > 0 ? Math.log10(clamped * 9 + 1) : 0;
  };

  const rmsPercent = Math.round(scale(sample.rms_level) * 100);
  const peakPercent = Math.round(scale(sample.peak_level) * 100);

  // Green means healthy here. The device-selection meter colours *low* levels
  // green and high levels red, which reads backwards: normal speech sits near
  // 60% and so appeared as a warning colour, while near-silence looked fine.
  const colour =
    sample.peak_level >= 0.98
      ? 'bg-red-500'
      : rmsPercent < 10
        ? 'bg-gray-400'
        : rmsPercent < 30
          ? 'bg-amber-500'
          : 'bg-green-500';

  return (
    <div className="flex items-center gap-2">
      <span className="text-[10px] text-gray-500 w-24 truncate" title={sample.device_name}>
        {sample.device_name}
      </span>
      <div className="flex-1 h-2 bg-gray-200 rounded-sm overflow-hidden relative">
        <div
          className={`h-full ${colour} transition-all duration-150 ease-out`}
          style={{ width: `${rmsPercent}%` }}
        />
        {peakPercent > rmsPercent && (
          <div
            className="absolute top-0 bottom-0 w-0.5 bg-gray-700/60"
            style={{ left: `${peakPercent}%` }}
          />
        )}
      </div>
      <span className="text-[10px] font-mono text-gray-500 w-8 text-right">{rmsPercent}%</span>
    </div>
  );
}
