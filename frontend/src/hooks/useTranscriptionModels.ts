import { useState, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { tr } from '@/lib/i18n';
import {
  cloudModelOptions,
  defaultModelKey,
  type CloudProviderInfo,
  type ModelOption,
  type TranscriptModelConfig,
} from '@/lib/transcription-model-options';

export interface RawModelInfo {
  name: string;
  size_mb: number;
  status: 'Available' | 'Missing' | { Downloading: { progress: number } } | { Error: string };
}

export type { ModelOption } from '@/lib/transcription-model-options';

/**
 * Custom hook for fetching and managing transcription models (Whisper, Parakeet and cloud).
 *
 * This hook centralizes the model fetching logic that was previously duplicated
 * in ImportAudioDialog and RetranscribeDialog components, and is also used by the
 * recordings library's transcribe dialog.
 *
 * @param transcriptModelConfig - User's saved model configuration from context
 * @returns Object containing available models, selected model key, loading state, and fetch function
 */
export function useTranscriptionModels(transcriptModelConfig: TranscriptModelConfig | undefined) {
  const [availableModels, setAvailableModels] = useState<ModelOption[]>([]);
  const [selectedModelKey, setSelectedModelKey] = useState<string>('');
  const [loadingModels, setLoadingModels] = useState(false);
  // True when every model source failed, so the dialog can explain the empty
  // list instead of silently leaving the user without a selectable model.
  const [loadError, setLoadError] = useState(false);
  // Track whether the user has manually changed the model selection
  const userSelectedRef = useRef(false);

  // Wrap setSelectedModelKey to track user-initiated changes
  const setSelectedModelKeyWithTracking = useCallback((key: string) => {
    userSelectedRef.current = true;
    setSelectedModelKey(key);
  }, []);

  const fetchModels = useCallback(async () => {
    setLoadingModels(true);
    setLoadError(false);
    const allModels: ModelOption[] = [];
    let failures = 0;

    // Fetch Whisper models
    try {
      const whisperModels = await invoke<RawModelInfo[]>('whisper_get_available_models');
      const availableWhisper = whisperModels
        .filter((m) => m.status === 'Available')
        .map((m) => ({
          provider: 'whisper' as const,
          name: m.name,
          displayName: `🏠 Whisper: ${m.name}`,
          size_mb: m.size_mb,
        }));
      allModels.push(...availableWhisper);
    } catch (err) {
      failures += 1;
      console.error('Failed to fetch Whisper models:', err);
    }

    // Fetch Parakeet models
    try {
      const parakeetModels = await invoke<RawModelInfo[]>('parakeet_get_available_models');
      const availableParakeet = parakeetModels
        .filter((m) => m.status === 'Available')
        .map((m) => ({
          provider: 'parakeet' as const,
          name: m.name,
          displayName: `⚡ Parakeet: ${m.name}`,
          size_mb: m.size_mb,
        }));
      allModels.push(...availableParakeet);
    } catch (err) {
      failures += 1;
      console.error('Failed to fetch Parakeet models:', err);
    }

    // Cloud models, listed after local ones so the default never silently
    // becomes an upload. Providers without a stored key are shown disabled.
    try {
      const providers = await invoke<CloudProviderInfo[]>('api_list_speech_providers');
      allModels.push(...cloudModelOptions(providers, transcriptModelConfig, tr('transcribeModels.needsKey')));
    } catch (err) {
      failures += 1;
      console.error('Failed to fetch cloud speech providers:', err);
    }

    setAvailableModels(allModels);
    // Only a total failure is attributed to loading: one missing provider simply
    // means fewer choices, and the no-models message covers the empty result.
    setLoadError(allModels.length === 0 && failures > 0);

    // Only set default model if user hasn't manually selected one
    if (!userSelectedRef.current) {
      const key = defaultModelKey(allModels, transcriptModelConfig);
      if (key) setSelectedModelKey(key);
    }

    setLoadingModels(false);
  }, [transcriptModelConfig]);

  // Reset user selection tracking (call when dialog opens fresh)
  const resetSelection = useCallback(() => {
    userSelectedRef.current = false;
  }, []);

  return {
    availableModels,
    selectedModelKey,
    setSelectedModelKey: setSelectedModelKeyWithTracking,
    loadingModels,
    loadError,
    fetchModels,
    resetSelection,
  };
}
