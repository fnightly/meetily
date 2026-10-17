'use client';

import { AlertCircle, Pause, Play } from 'lucide-react';
import { useAudioPlayer } from '@/hooks/useAudioPlayer';
import { useI18n } from '@/contexts/I18nContext';
import { formatDuration } from '@/lib/recordings';

interface RecordingAudioPreviewProps {
  /** Absolute audio path from `RecordingEntry.audio_path`. */
  audioPath: string;
}

/**
 * Compact inline player for one recording.
 *
 * Playback goes through `useAudioPlayer`, which reads the file with
 * `read_audio_file`; the Tauri asset protocol does not cover the recordings
 * folder, so `convertFileSrc` would fail here.
 */
export function RecordingAudioPreview({ audioPath }: RecordingAudioPreviewProps) {
  const { t } = useI18n();
  const { isPlaying, currentTime, duration, error, play, pause, seek } = useAudioPlayer(audioPath);

  const toggle = () => {
    if (isPlaying) pause();
    else void play();
  };

  return (
    <div className="surface-card mt-1.5 px-3 py-2">
      <div className="flex items-center gap-2.5">
        <button
          type="button"
          onClick={toggle}
          aria-label={isPlaying ? t('library.pause') : t('library.play')}
          title={t('library.playTitle')}
          className="inline-flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full bg-blue-600 text-white transition-colors hover:bg-blue-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
        >
          {isPlaying ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
        </button>

        <span className="flex-shrink-0 text-xs tabular-nums text-slate-500">
          {formatDuration(currentTime)} / {formatDuration(duration)}
        </span>

        <input
          type="range"
          min={0}
          max={duration || 0}
          step={0.1}
          value={Math.min(currentTime, duration || 0)}
          onChange={(event) => void seek(Number(event.target.value))}
          aria-label={t('library.playTitle')}
          disabled={duration <= 0}
          className="h-4 min-w-0 flex-1 cursor-pointer accent-blue-600 disabled:cursor-not-allowed disabled:opacity-50"
        />
      </div>

      {error && (
        <p className="mt-1.5 flex items-center gap-1.5 text-xs text-red-700">
          <AlertCircle className="h-3.5 w-3.5 flex-shrink-0" />
          {t('library.audioPlayFailed')}
        </p>
      )}
    </div>
  );
}
