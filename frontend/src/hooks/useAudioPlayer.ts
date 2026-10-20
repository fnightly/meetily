import { useEffect, useRef, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';

/**
 * Inline audio player backed by an HTMLAudioElement.
 *
 * Earlier this hook read the whole file through `read_audio_file` and decoded it
 * into an AudioBuffer before it could show a duration. A one-hour meeting is
 * tens of megabytes on disk and hundreds of megabytes once decoded, so the
 * player stayed at 0:00 / 0:00 for a long time and kept the decoded audio in
 * memory. The asset protocol streams the file instead: metadata (duration)
 * arrives immediately and playback reads only what it needs.
 *
 * The backend allows each recording file in the asset scope when it resolves
 * the audio path or lists the library, because the recordings folder is not
 * necessarily under $APPDATA.
 */
export const useAudioPlayer = (audioPath: string | null) => {
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  useEffect(() => {
    if (!audioPath) {
      setDuration(0);
      setCurrentTime(0);
      setIsPlaying(false);
      setError(null);
      return;
    }

    let cancelled = false;
    const audio = new Audio();
    audio.preload = 'metadata';
    audioRef.current = audio;
    setDuration(0);
    setCurrentTime(0);
    setIsPlaying(false);
    setError(null);

    const onLoadedMetadata = () => {
      if (cancelled) return;
      const nextDuration = Number.isFinite(audio.duration) ? audio.duration : 0;
      setDuration(nextDuration);
      setError(null);
    };
    const onTimeUpdate = () => {
      if (!cancelled) setCurrentTime(audio.currentTime);
    };
    const onPlay = () => {
      if (!cancelled) setIsPlaying(true);
    };
    const onPause = () => {
      if (!cancelled) setIsPlaying(false);
    };
    const onEnded = () => {
      if (cancelled) return;
      setIsPlaying(false);
      setCurrentTime(0);
    };
    const onError = () => {
      if (cancelled) return;
      setIsPlaying(false);
      setError('Failed to load audio file');
    };

    audio.addEventListener('loadedmetadata', onLoadedMetadata);
    audio.addEventListener('durationchange', onLoadedMetadata);
    audio.addEventListener('timeupdate', onTimeUpdate);
    audio.addEventListener('play', onPlay);
    audio.addEventListener('pause', onPause);
    audio.addEventListener('ended', onEnded);
    audio.addEventListener('error', onError);
    audio.src = convertFileSrc(audioPath);
    audio.load();

    return () => {
      cancelled = true;
      audio.removeEventListener('loadedmetadata', onLoadedMetadata);
      audio.removeEventListener('durationchange', onLoadedMetadata);
      audio.removeEventListener('timeupdate', onTimeUpdate);
      audio.removeEventListener('play', onPlay);
      audio.removeEventListener('pause', onPause);
      audio.removeEventListener('ended', onEnded);
      audio.removeEventListener('error', onError);
      audio.pause();
      audio.src = '';
      audioRef.current = null;
    };
  }, [audioPath]);

  const play = async () => {
    const audio = audioRef.current;
    if (!audio) return;
    try {
      await audio.play();
      setError(null);
    } catch (e) {
      console.error('Error during playback:', e);
      setError('Failed to play audio');
      setIsPlaying(false);
    }
  };

  const pause = () => {
    audioRef.current?.pause();
  };

  const seek = async (time: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    const total = Number.isFinite(audio.duration) ? audio.duration : 0;
    const next = Math.min(Math.max(time, 0), total || 0);
    audio.currentTime = next;
    setCurrentTime(next);
  };

  return { isPlaying, currentTime, duration, error, play, pause, seek };
};
