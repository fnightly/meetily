/**
 * The model choices offered when transcribing a saved recording or an imported
 * file, and how a chosen one becomes batch command arguments.
 *
 * Local models come from the Whisper/Parakeet managers; cloud models come from
 * the speech-provider catalog (`api_list_speech_providers`). Kept free of React
 * and Tauri so the rules can be tested directly.
 */

import type { BatchTranscriptionTarget } from './transcription-target';

export interface ModelOption {
  /** `whisper` / `parakeet` run locally; any other id is a cloud provider. */
  provider: 'whisper' | 'parakeet' | 'openai' | 'groq' | 'siliconflow' | (string & {});
  name: string;
  displayName: string;
  size_mb: number;
  /** Shown but not selectable: a cloud provider whose API key is not saved yet. */
  disabled?: boolean;
}

/** The catalog fields this module reads. */
export interface CloudProviderInfo {
  id: string;
  display_name: string;
  kind: string;
  selectable: boolean;
  has_api_key: boolean;
  models: string[];
}

export interface TranscriptModelConfig {
  provider?: string;
  model?: string;
}

export const modelKey = (m: Pick<ModelOption, 'provider' | 'name'>) => `${m.provider}:${m.name}`;

/**
 * Every model of every usable cloud provider.
 *
 * All of a provider's models are listed, not just its first, so the user can
 * pick e.g. TeleSpeech instead of SenseVoice for one recording. A model saved
 * in settings that the catalog does not list is kept as well. Providers with no
 * stored key are still listed, disabled, so the user can see why they cannot
 * pick them instead of the option silently missing.
 */
export function cloudModelOptions(
  providers: CloudProviderInfo[] | null | undefined,
  config: TranscriptModelConfig | undefined,
  needsKeyLabel: string,
): ModelOption[] {
  const options: ModelOption[] = [];
  for (const p of providers ?? []) {
    if (p.kind !== 'cloud' || !p.selectable) continue;
    const models = [...p.models];
    const saved = config?.provider === p.id ? config.model?.trim() : undefined;
    if (saved && !models.includes(saved)) models.unshift(saved);
    for (const model of models) {
      options.push({
        provider: p.id,
        name: model,
        displayName: p.has_api_key
          ? `☁️ ${p.display_name}: ${model}`
          : `☁️ ${p.display_name}: ${model} (${needsKeyLabel})`,
        size_mb: 0,
        disabled: !p.has_api_key,
      });
    }
  }
  return options;
}

/**
 * The model to preselect: the one saved in settings when it is offered and
 * enabled, else the first enabled option. `localWhisper` in settings is the
 * `whisper` provider in the list.
 */
export function defaultModelKey(
  models: ModelOption[],
  config: TranscriptModelConfig | undefined,
): string | null {
  const enabled = models.filter((m) => !m.disabled);
  const configuredProvider = config?.provider === 'localWhisper' ? 'whisper' : config?.provider || '';
  const configuredModel = config?.model || '';
  const match = enabled.find((m) => m.provider === configuredProvider && m.name === configuredModel);
  if (match) return modelKey(match);
  return enabled.length > 0 ? modelKey(enabled[0]) : null;
}

/**
 * Batch command arguments for an explicitly chosen model.
 *
 * The provider is always sent: a null provider means "the configured one",
 * which would send a recording to the saved cloud provider even though the
 * user picked a local model. Parakeet takes no language hint.
 */
export function batchTargetForModel(
  base: BatchTranscriptionTarget,
  option: Pick<ModelOption, 'provider' | 'name'>,
): BatchTranscriptionTarget {
  return {
    provider: option.provider,
    model: option.name,
    language: option.provider === 'parakeet' ? null : base.language,
  };
}
