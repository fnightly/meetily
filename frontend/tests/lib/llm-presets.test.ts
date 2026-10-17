import { describe, expect, test } from 'bun:test';
import { LLM_PRESETS, applyPreset, draftProblem, presetForEndpoint } from '../../src/lib/llm-presets';

describe('llm presets', () => {
  test('ids are unique and endpoints are base URLs', () => {
    const ids = LLM_PRESETS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const p of LLM_PRESETS) {
      expect(p.endpoint).toMatch(/^https?:\/\//);
      expect(p.endpoint.endsWith('/chat/completions')).toBe(false);
      if (p.kind === 'cloud') {
        expect(p.endpoint.startsWith('https://')).toBe(true);
        expect(p.requiresKey).toBe(true);
        expect(p.models.length).toBeGreaterThan(0);
      }
    }
  });

  test('matches a saved endpoint regardless of trailing slash or suffix', () => {
    expect(presetForEndpoint('https://api.deepseek.com/v1/')?.id).toBe('deepseek');
    expect(presetForEndpoint('https://API.deepseek.com/v1/chat/completions')?.id).toBe('deepseek');
    expect(presetForEndpoint('https://example.com/v1')).toBeUndefined();
    expect(presetForEndpoint('')).toBeUndefined();
  });

  test('switching vendor clears the key and picks the vendor default model', () => {
    const kimi = LLM_PRESETS.find((p) => p.id === 'kimi')!;
    const next = applyPreset({ endpoint: 'https://api.deepseek.com/v1', model: 'deepseek-chat', apiKey: 'sk-ds' }, kimi);
    expect(next).toEqual({ endpoint: kimi.endpoint, model: kimi.models[0], apiKey: '' });
  });

  test('re-applying the same vendor keeps key and a valid model', () => {
    const ds = LLM_PRESETS.find((p) => p.id === 'deepseek')!;
    const next = applyPreset({ endpoint: ds.endpoint + '/', model: 'deepseek-reasoner', apiKey: 'sk' }, ds);
    expect(next.apiKey).toBe('sk');
    expect(next.model).toBe('deepseek-reasoner');
  });

  test('local presets keep the typed model', () => {
    const lm = LLM_PRESETS.find((p) => p.id === 'lmstudio')!;
    expect(applyPreset({ endpoint: '', model: 'qwen2.5-7b', apiKey: '' }, lm).model).toBe('qwen2.5-7b');
  });

  test('draft validation', () => {
    expect(draftProblem({ endpoint: '', model: 'm', apiKey: '' })).toBe('endpoint');
    expect(draftProblem({ endpoint: 'api.x.com', model: 'm', apiKey: '' })).toBe('endpointScheme');
    expect(draftProblem({ endpoint: 'http://localhost:1234/v1', model: ' ', apiKey: '' })).toBe('model');
    expect(draftProblem({ endpoint: 'https://api.deepseek.com/v1', model: 'deepseek-chat', apiKey: '' })).toBe('apiKey');
    expect(draftProblem({ endpoint: 'http://localhost:1234/v1', model: 'x', apiKey: '' })).toBeNull();
  });
});
