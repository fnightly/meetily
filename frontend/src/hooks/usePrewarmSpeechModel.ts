import { useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** Don't ask the backend more than once per this window. */
const MIN_INTERVAL_MS = 45_000;
let lastRequest = 0;

/**
 * Preload the speech model when the user shows intent to record (pointer over,
 * or keyboard focus on, a record button). Loading takes seconds for the larger
 * models; starting it a moment before the click hides most of that wait.
 * The backend ignores the call while recording, for cloud providers, or when
 * the model is already loaded.
 */
export function usePrewarmSpeechModel() {
  const inFlight = useRef(false);

  const prewarm = useCallback(() => {
    const now = Date.now();
    if (inFlight.current || now - lastRequest < MIN_INTERVAL_MS) return;
    lastRequest = now;
    inFlight.current = true;
    invoke('transcription_prewarm')
      .catch(() => {
        /* best effort: the start path validates and loads anyway */
      })
      .finally(() => {
        inFlight.current = false;
      });
  }, []);

  return { onPointerEnter: prewarm, onFocus: prewarm };
}
