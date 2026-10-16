import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/**
 * Speech-to-text provider catalog, as reported by the backend.
 *
 * The backend owns provider capabilities (`src-tauri/src/audio/transcription/
 * catalog.rs`); this hook only mirrors them. Keeping the list server-side is
 * what prevents the UI from offering an option the backend will refuse (the
 * previous hardcoded `modelOptions` map drifted out of sync exactly that way).
 */

export type ProviderKind = 'local' | 'cloud';
export type ProviderStatus = 'available' | 'not_implemented';

export interface SpeechProviderView {
  id: string;
  display_name: string;
  kind: ProviderKind;
  status: ProviderStatus;
  /** `null` = any language; a list = restricted to those ISO 639-1 codes. */
  supported_languages: string[] | null;
  requires_api_key: boolean;
  has_api_key: boolean;
  models: string[];
  notes: string;
  api_key_url: string | null;
  /** Whether this provider may be selected for a recording right now. */
  selectable: boolean;
}

export interface ActiveSpeechSettings {
  provider: string;
  model: string;
  language: string | null;
  has_api_key: boolean;
}

export function useSpeechProviders() {
  const [providers, setProviders] = useState<SpeechProviderView[]>([]);
  const [active, setActive] = useState<ActiveSpeechSettings | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setIsLoading(true);
    setError(null);
    try {
      const [catalog, current] = await Promise.all([
        invoke<SpeechProviderView[]>('api_list_speech_providers'),
        invoke<ActiveSpeechSettings>('api_get_active_speech_settings').catch(() => null),
      ]);
      setProviders(catalog);
      setActive(current);
      return catalog;
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setError(message);
      console.error('Failed to load speech provider catalog:', e);
      return [];
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /**
   * Check a provider/language pair before committing to it.
   *
   * Returns `null` when acceptable, or the backend's explanation when not.
   */
  const validateSelection = useCallback(
    async (providerId: string, language: string | null): Promise<string | null> => {
      try {
        await invoke('api_validate_speech_selection', {
          provider: providerId,
          language: language ?? null,
        });
        return null;
      } catch (e) {
        return e instanceof Error ? e.message : String(e);
      }
    },
    [],
  );

  /** Language codes the provider accepts, or `null` when unrestricted. */
  const languagesFor = useCallback(async (providerId: string): Promise<string[] | null> => {
    try {
      return await invoke<string[] | null>('api_get_provider_languages', { provider: providerId });
    } catch {
      return null;
    }
  }, []);

  return { providers, active, isLoading, error, refresh, validateSelection, languagesFor };
}
