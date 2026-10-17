'use client';

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { formatSpeakerLabel } from '@/lib/speaker-labels';
import type { TranslateParams } from '@/lib/i18n';

type Translate = (key: string, params?: TranslateParams) => string;

/** Stored label (`speaker_1`) → user-given display name. */
export type SpeakerNames = Record<string, string>;

/** Display text for a label: the user's name for it, else "说话人 N". */
export function displaySpeaker(label: string, t: Translate, names?: SpeakerNames | null): string {
  const named = names?.[label]?.trim();
  return named ? named : formatSpeakerLabel(label, t);
}

/** Distinct speaker labels in first-appearance order. */
export function speakerLabelsIn(segments: Array<{ speaker?: string | null }>): string[] {
  const seen: string[] = [];
  for (const s of segments) {
    if (s.speaker && !seen.includes(s.speaker)) seen.push(s.speaker);
  }
  return seen;
}

export async function fetchSpeakerNames(meetingId: string): Promise<SpeakerNames> {
  try {
    return (await invoke<SpeakerNames>('api_get_speaker_names', { meetingId })) ?? {};
  } catch {
    return {};
  }
}

interface SpeakerNamesValue {
  meetingId: string | null;
  names: SpeakerNames;
  /** Ask to rename a label; null when renaming is not available here. */
  requestRename: ((label: string) => void) | null;
  rename: (label: string, name: string | null) => Promise<void>;
}

const SpeakerNamesContext = createContext<SpeakerNamesValue>({
  meetingId: null,
  names: {},
  requestRename: null,
  rename: async () => {},
});

export const SPEAKER_NAMES_EVENT = 'meetily:speaker-names-changed';

/**
 * Loads and edits one meeting's speaker names for everything rendered inside it.
 *
 * The transcript rows read names from context instead of props so the memoized
 * row component does not need a new prop chain through the virtualizer.
 */
export function SpeakerNamesProvider({
  meetingId,
  onRequestRename,
  children,
}: {
  meetingId: string | null;
  onRequestRename?: (label: string) => void;
  children: ReactNode;
}) {
  const [names, setNames] = useState<SpeakerNames>({});

  useEffect(() => {
    let alive = true;
    if (!meetingId) {
      setNames({});
      return;
    }
    void fetchSpeakerNames(meetingId).then((n) => alive && setNames(n));
    const onChanged = (e: Event) => {
      const detail = (e as CustomEvent<{ meetingId: string; names: SpeakerNames }>).detail;
      if (detail?.meetingId === meetingId) setNames(detail.names);
    };
    window.addEventListener(SPEAKER_NAMES_EVENT, onChanged);
    return () => {
      alive = false;
      window.removeEventListener(SPEAKER_NAMES_EVENT, onChanged);
    };
  }, [meetingId]);

  const rename = useCallback(async (label: string, name: string | null) => {
    if (!meetingId) return;
    const next = await invoke<SpeakerNames>('api_set_speaker_name', { meetingId, speaker: label, name });
    setNames(next);
    window.dispatchEvent(new CustomEvent(SPEAKER_NAMES_EVENT, { detail: { meetingId, names: next } }));
  }, [meetingId]);

  const value = useMemo(
    () => ({ meetingId, names, requestRename: onRequestRename ?? null, rename }),
    [meetingId, names, onRequestRename, rename],
  );
  return <SpeakerNamesContext.Provider value={value}>{children}</SpeakerNamesContext.Provider>;
}

export function useSpeakerNames(): SpeakerNamesValue {
  return useContext(SpeakerNamesContext);
}
