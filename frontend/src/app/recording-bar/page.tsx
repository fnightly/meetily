'use client';

/**
 * Floating recording bar.
 *
 * Rendered in a small always-on-top window that the backend shows while a
 * recording is live and the main window is not in front
 * (src-tauri/src/recording_bar.rs). It shows that the meeting is still being
 * recorded, for how long, and lets the user pause, resume or stop without
 * switching back. Stopping goes through the same path as the tray, so the
 * meeting is finished and saved by the main window in the background.
 */

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Maximize2, Pause, Play, Square } from 'lucide-react';
import { tr } from '@/lib/i18n';
import { formatClock } from '@/lib/recording-bar';

interface BackendRecordingState {
  is_recording: boolean;
  is_paused: boolean;
  active_duration: number | null;
}

export default function RecordingBar() {
  const [state, setState] = useState<BackendRecordingState>({
    is_recording: false,
    is_paused: false,
    active_duration: null,
  });
  const [busy, setBusy] = useState<'pause' | 'stop' | null>(null);
  const [level, setLevel] = useState(0);
  const mounted = useRef(true);

  // Poll rather than only listen: the window may be created mid-recording and
  // the duration has to tick anyway.
  useEffect(() => {
    mounted.current = true;
    const sync = async () => {
      try {
        const next = await invoke<BackendRecordingState>('get_recording_state');
        if (mounted.current && next) setState(next);
      } catch (error) {
        console.error('[RecordingBar] Failed to read recording state:', error);
      }
    };
    void sync();
    const timer = setInterval(sync, 500);

    const unlisteners: Array<Promise<() => void>> = [
      listen('recording-paused', sync),
      listen('recording-resumed', sync),
      listen('recording-started', () => {
        setBusy(null);
        void sync();
      }),
      listen('recording-stopped', () => {
        setBusy(null);
        void sync();
      }),
      listen<{ levels: Array<{ rms_level: number }> }>('audio-levels', (event) => {
        const levels = event.payload?.levels ?? [];
        if (levels.length === 0) return;
        const loudest = Math.max(...levels.map((l) => l.rms_level ?? 0));
        // Same curve as the main window's bars: perceived loudness is not linear in RMS.
        setLevel(Math.min(1, Math.sqrt(Math.max(0, loudest)) * 1.4));
      }),
    ];

    return () => {
      mounted.current = false;
      clearInterval(timer);
      unlisteners.forEach((p) => p.then((stop) => stop()).catch(() => {}));
    };
  }, []);

  const paused = state.is_paused;

  const togglePause = async () => {
    if (busy) return;
    setBusy('pause');
    try {
      await invoke(paused ? 'resume_recording' : 'pause_recording');
      const next = await invoke<BackendRecordingState>('get_recording_state');
      if (next) setState(next);
    } catch (error) {
      console.error('[RecordingBar] Pause/resume failed:', error);
    } finally {
      setBusy(null);
    }
  };

  const stop = async () => {
    if (busy) return;
    setBusy('stop');
    try {
      await invoke('recording_bar_stop');
    } catch (error) {
      console.error('[RecordingBar] Stop failed:', error);
      setBusy(null);
    }
  };

  const openMain = () => {
    invoke('recording_bar_open_main').catch((error) =>
      console.error('[RecordingBar] Failed to open the main window:', error),
    );
  };

  const bars = [0.55, 1, 0.75].map((weight) =>
    paused ? 0.15 : Math.max(0.15, Math.min(1, level * weight)),
  );

  return (
    <div className="h-screen w-screen p-1 select-none">
      <div
        data-tauri-drag-region
        className="flex h-full w-full items-center gap-3 rounded-full bg-slate-900/95 pl-4 pr-1.5 text-white shadow-lg ring-1 ring-white/10"
        onDoubleClick={openMain}
      >
        <span className="relative flex h-2.5 w-2.5 shrink-0" aria-hidden="true">
          {!paused && (
            <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-red-500 opacity-60" />
          )}
          <span
            className={`relative inline-flex h-2.5 w-2.5 rounded-full ${paused ? 'bg-amber-400' : 'bg-red-500'}`}
          />
        </span>

        <div data-tauri-drag-region className="flex min-w-0 flex-1 items-baseline gap-2" role="status" aria-live="polite">
          <span data-tauri-drag-region className="text-xs font-medium text-slate-300">
            {paused ? tr('recordingBar.paused') : tr('recordingBar.recording')}
          </span>
          <span data-tauri-drag-region className="font-mono text-sm tabular-nums">
            {formatClock(state.active_duration ?? 0)}
          </span>
        </div>

        <div data-tauri-drag-region className="flex h-4 items-end gap-0.5" aria-hidden="true">
          {bars.map((height, index) => (
            <span
              key={index}
              className="w-1 rounded-sm bg-emerald-400 transition-[height] duration-150"
              style={{ height: `${Math.round(height * 100)}%` }}
            />
          ))}
        </div>

        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={togglePause}
            disabled={busy !== null || !state.is_recording}
            title={paused ? tr('recordingBar.resume') : tr('recordingBar.pause')}
            aria-label={paused ? tr('recordingBar.resume') : tr('recordingBar.pause')}
            className="flex h-9 w-9 items-center justify-center rounded-full bg-white/10 hover:bg-white/20 disabled:opacity-50"
          >
            {paused ? <Play className="h-4 w-4" /> : <Pause className="h-4 w-4" />}
          </button>
          <button
            type="button"
            onClick={stop}
            disabled={busy !== null || !state.is_recording}
            title={tr('recordingBar.stop')}
            aria-label={tr('recordingBar.stop')}
            className="flex h-9 w-9 items-center justify-center rounded-full bg-red-500 hover:bg-red-600 disabled:opacity-50"
          >
            <Square className="h-3.5 w-3.5 fill-current" />
          </button>
          <button
            type="button"
            onClick={openMain}
            title={tr('recordingBar.open')}
            aria-label={tr('recordingBar.open')}
            className="flex h-9 w-9 items-center justify-center rounded-full text-slate-300 hover:bg-white/10 hover:text-white"
          >
            <Maximize2 className="h-4 w-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
