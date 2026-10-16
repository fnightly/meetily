'use client';

import React, { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { AlertTriangle, Mic } from 'lucide-react';
import {
  LevelTracker,
  formatDbfs,
  guidanceFor,
  type LevelDiagnosis,
  type LevelSample,
} from '@/lib/audio-levels';

interface AudioLevelUpdate {
  levels: Array<{
    device_name: string;
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
  const [diagnosis, setDiagnosis] = useState<LevelDiagnosis | null>(null);
  const [latest, setLatest] = useState<LevelSample[]>([]);
  const trackerRef = useRef(new LevelTracker());

  useEffect(() => {
    if (!active) {
      trackerRef.current.reset();
      setDiagnosis(null);
      setLatest([]);
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

  const warning = diagnosis ? guidanceFor(diagnosis.verdict, diagnosis.deviceName) : null;
  const isProblem = diagnosis !== null && diagnosis.verdict !== 'healthy';

  return (
    <div className={`space-y-2 ${className}`}>
      <div className="flex items-center gap-2">
        <Mic className="w-4 h-4 text-gray-500 shrink-0" />
        <div className="flex-1 space-y-1">
          {latest.length === 0 ? (
            <p className="text-xs text-gray-500">Waiting for audio…</p>
          ) : (
            latest.map((sample) => (
              <LevelBar key={sample.device_name} sample={sample} />
            ))
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
