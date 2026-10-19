import { describe, expect, test } from 'bun:test';
import {
  batchTargetForModel,
  cloudModelOptions,
  defaultModelKey,
  type CloudProviderInfo,
} from '../../src/lib/transcription-model-options';

const providers: CloudProviderInfo[] = [
  { id: 'parakeet', display_name: 'Parakeet', kind: 'local', selectable: true, has_api_key: false, models: ['p'] },
  { id: 'deepgram', display_name: 'Deepgram', kind: 'cloud', selectable: false, has_api_key: true, models: ['nova-2'] },
  {
    id: 'siliconflow',
    display_name: 'SiliconFlow',
    kind: 'cloud',
    selectable: true,
    has_api_key: true,
    models: ['FunAudioLLM/SenseVoiceSmall', 'TeleAI/TeleSpeechASR'],
  },
  { id: 'openai', display_name: 'OpenAI', kind: 'cloud', selectable: true, has_api_key: false, models: ['whisper-1'] },
];

describe('cloudModelOptions', () => {
  test('lists every model of each usable cloud provider', () => {
    const options = cloudModelOptions(providers, undefined, 'needs key');
    expect(options.map((o) => `${o.provider}:${o.name}`)).toEqual([
      'siliconflow:FunAudioLLM/SenseVoiceSmall',
      'siliconflow:TeleAI/TeleSpeechASR',
      'openai:whisper-1',
    ]);
  });

  test('shows a provider without a key as disabled, with the reason', () => {
    const openai = cloudModelOptions(providers, undefined, 'needs key').find((o) => o.provider === 'openai');
    expect(openai?.disabled).toBe(true);
    expect(openai?.displayName).toContain('needs key');
  });

  test('keeps a saved model the catalog does not list', () => {
    const options = cloudModelOptions(providers, { provider: 'siliconflow', model: 'custom/asr' }, 'k');
    expect(options[0]).toMatchObject({ provider: 'siliconflow', name: 'custom/asr', disabled: false });
  });
});

describe('defaultModelKey', () => {
  const models = [
    { provider: 'whisper', name: 'small', displayName: '', size_mb: 1 },
    ...cloudModelOptions(providers, undefined, 'k'),
  ];

  test('prefers the saved model, mapping localWhisper to whisper', () => {
    expect(defaultModelKey(models, { provider: 'localWhisper', model: 'small' })).toBe('whisper:small');
    expect(defaultModelKey(models, { provider: 'siliconflow', model: 'TeleAI/TeleSpeechASR' })).toBe(
      'siliconflow:TeleAI/TeleSpeechASR',
    );
  });

  test('never preselects a disabled option', () => {
    const cloudOnly = models.filter((m) => m.provider === 'openai');
    expect(defaultModelKey(cloudOnly, { provider: 'openai', model: 'whisper-1' })).toBeNull();
  });
});

describe('batchTargetForModel', () => {
  const base = { provider: 'groq', model: 'whisper-large-v3', language: 'zh' };

  test('sends the chosen provider and model, keeping the language', () => {
    expect(batchTargetForModel(base, { provider: 'siliconflow', name: 'TeleAI/TeleSpeechASR' })).toEqual({
      provider: 'siliconflow',
      model: 'TeleAI/TeleSpeechASR',
      language: 'zh',
    });
  });

  test('sends local Whisper explicitly so a saved cloud provider is not used', () => {
    expect(batchTargetForModel(base, { provider: 'whisper', name: 'small' }).provider).toBe('whisper');
  });

  test('drops the language for Parakeet', () => {
    expect(batchTargetForModel(base, { provider: 'parakeet', name: 'p' }).language).toBeNull();
  });
});
