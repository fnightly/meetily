import { describe, expect, test } from 'bun:test';
import { batchTranscriptionTarget } from '../../src/lib/transcription-target';

const settings = (provider: string, model = 'tiny', language: string | null = null) =>
  ({ provider, model, language });

describe('batchTranscriptionTarget', () => {
  test('missing settings leave every argument to the command defaults', () => {
    expect(batchTranscriptionTarget(null)).toEqual({ provider: null, model: null, language: null });
    expect(batchTranscriptionTarget(undefined)).toEqual({ provider: null, model: null, language: null });
  });

  test('local Whisper is the default engine, so its id is not sent', () => {
    expect(batchTranscriptionTarget(settings('localWhisper', 'base', 'auto'))).toEqual({
      provider: null,
      model: 'base',
      language: null,
    });
  });

  test('Parakeet is selected explicitly, because the command otherwise runs Whisper', () => {
    expect(batchTranscriptionTarget(settings('parakeet', 'parakeet-tdt-0.6b-v3-int8'))).toEqual({
      provider: 'parakeet',
      model: 'parakeet-tdt-0.6b-v3-int8',
      language: null,
    });
  });

  test('cloud provider ids pass through for the batch path to resolve', () => {
    expect(batchTranscriptionTarget(settings('deepgram', 'nova-3', 'zh')).provider).toBe('deepgram');
  });

  test('automatic language preferences are not sent as a language code', () => {
    expect(batchTranscriptionTarget(settings('localWhisper', 'tiny', 'auto')).language).toBeNull();
    expect(batchTranscriptionTarget(settings('localWhisper', 'tiny', 'auto-translate')).language).toBeNull();
    expect(batchTranscriptionTarget(settings('localWhisper', 'tiny', null)).language).toBeNull();
  });

  test('a concrete language is forwarded', () => {
    expect(batchTranscriptionTarget(settings('localWhisper', 'tiny', 'zh')).language).toBe('zh');
    expect(batchTranscriptionTarget(settings('localWhisper', 'tiny', 'en')).language).toBe('en');
  });

  test('a blank model falls back to the provider default', () => {
    expect(batchTranscriptionTarget(settings('parakeet', '   ')).model).toBeNull();
  });
});
