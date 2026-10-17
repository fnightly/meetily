/**
 * One-click presets for OpenAI-compatible summary endpoints.
 *
 * Every entry is driven through the existing `custom-openai` provider, so no
 * backend change is needed to add a vendor: the Rust client already posts to
 * `{endpoint}/chat/completions` with a Bearer key. Presets only pre-fill the
 * endpoint and suggest model ids; the user can still edit both.
 *
 * Model ids are suggestions, not a whitelist — vendors rename models often, so
 * the field stays free text and these only populate the picker.
 */

export type LlmPresetKind = 'cloud' | 'local';

export interface LlmPreset {
  id: string;
  /** Display name (brand names are not translated). */
  label: string;
  kind: LlmPresetKind;
  /** Base URL, without the trailing `/chat/completions`. */
  endpoint: string;
  /** Suggested model ids; the first is the default. */
  models: string[];
  /** Where to create an API key (cloud only). */
  keyUrl?: string;
  /** i18n key for a one-line hint shown under the picker. */
  noteKey?: string;
  /** Whether an API key is required for requests to succeed. */
  requiresKey: boolean;
}

export const LLM_PRESETS: LlmPreset[] = [
  {
    id: 'deepseek',
    label: 'DeepSeek',
    kind: 'cloud',
    endpoint: 'https://api.deepseek.com/v1',
    models: ['deepseek-chat', 'deepseek-reasoner'],
    keyUrl: 'https://platform.deepseek.com/api_keys',
    requiresKey: true,
  },
  {
    id: 'qwen',
    label: '通义千问 Qwen',
    kind: 'cloud',
    endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    models: ['qwen-plus', 'qwen-max', 'qwen-turbo', 'qwen-long'],
    keyUrl: 'https://bailian.console.aliyun.com/?apiKey=1',
    noteKey: 'llmPreset.note.qwen',
    requiresKey: true,
  },
  {
    id: 'kimi',
    label: 'Kimi (Moonshot)',
    kind: 'cloud',
    endpoint: 'https://api.moonshot.cn/v1',
    models: ['kimi-latest', 'moonshot-v1-128k', 'moonshot-v1-32k'],
    keyUrl: 'https://platform.moonshot.cn/console/api-keys',
    noteKey: 'llmPreset.note.kimi',
    requiresKey: true,
  },
  {
    id: 'zhipu',
    label: '智谱 GLM',
    kind: 'cloud',
    endpoint: 'https://open.bigmodel.cn/api/paas/v4',
    models: ['glm-4-plus', 'glm-4-air', 'glm-4-flash'],
    keyUrl: 'https://open.bigmodel.cn/usercenter/apikeys',
    requiresKey: true,
  },
  {
    id: 'siliconflow',
    label: '硅基流动 SiliconFlow',
    kind: 'cloud',
    endpoint: 'https://api.siliconflow.cn/v1',
    models: ['deepseek-ai/DeepSeek-V3', 'Qwen/Qwen2.5-72B-Instruct', 'THUDM/glm-4-9b-chat'],
    keyUrl: 'https://cloud.siliconflow.cn/account/ak',
    noteKey: 'llmPreset.note.siliconflow',
    requiresKey: true,
  },
  {
    id: 'doubao',
    label: '豆包 (火山方舟)',
    kind: 'cloud',
    endpoint: 'https://ark.cn-beijing.volces.com/api/v3',
    models: ['doubao-1-5-pro-32k-250115', 'doubao-1-5-lite-32k-250115'],
    keyUrl: 'https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey',
    noteKey: 'llmPreset.note.doubao',
    requiresKey: true,
  },
  {
    id: 'gemini',
    label: 'Google Gemini',
    kind: 'cloud',
    endpoint: 'https://generativelanguage.googleapis.com/v1beta/openai',
    models: ['gemini-2.5-flash', 'gemini-2.5-pro'],
    keyUrl: 'https://aistudio.google.com/apikey',
    requiresKey: true,
  },
  {
    id: 'lmstudio',
    label: 'LM Studio',
    kind: 'local',
    endpoint: 'http://localhost:1234/v1',
    models: [],
    noteKey: 'llmPreset.note.local',
    requiresKey: false,
  },
  {
    id: 'llamacpp',
    label: 'llama.cpp server',
    kind: 'local',
    endpoint: 'http://localhost:8080/v1',
    models: [],
    noteKey: 'llmPreset.note.local',
    requiresKey: false,
  },
  {
    id: 'vllm',
    label: 'vLLM',
    kind: 'local',
    endpoint: 'http://localhost:8000/v1',
    models: [],
    noteKey: 'llmPreset.note.local',
    requiresKey: false,
  },
];

function normalizeEndpoint(url: string): string {
  return url.trim().replace(/\/+$/, '').replace(/\/chat\/completions$/, '').toLowerCase();
}

/** The preset whose endpoint matches, so a saved config shows its vendor. */
export function presetForEndpoint(endpoint: string | null | undefined): LlmPreset | undefined {
  if (!endpoint?.trim()) return undefined;
  const target = normalizeEndpoint(endpoint);
  return LLM_PRESETS.find((p) => normalizeEndpoint(p.endpoint) === target);
}

export interface CustomEndpointDraft {
  endpoint: string;
  model: string;
  apiKey: string;
}

/**
 * Apply a preset to the current draft.
 *
 * The API key is cleared when the endpoint changes vendor: a DeepSeek key sent
 * to Moonshot would be leaked to a third party and fail anyway. The model is
 * kept when it is one of the preset's suggestions, otherwise replaced by the
 * preset's default (local presets keep whatever the user typed).
 */
export function applyPreset(draft: CustomEndpointDraft, preset: LlmPreset): CustomEndpointDraft {
  const sameEndpoint = normalizeEndpoint(draft.endpoint) === normalizeEndpoint(preset.endpoint);
  const keepModel = preset.models.length === 0 || preset.models.includes(draft.model.trim());
  return {
    endpoint: preset.endpoint,
    model: keepModel ? draft.model : preset.models[0],
    apiKey: sameEndpoint ? draft.apiKey : '',
  };
}

/** Whether the draft can be saved/tested for this endpoint. */
export function draftProblem(draft: CustomEndpointDraft): 'endpoint' | 'endpointScheme' | 'model' | 'apiKey' | null {
  const endpoint = draft.endpoint.trim();
  if (!endpoint) return 'endpoint';
  if (!/^https?:\/\//i.test(endpoint)) return 'endpointScheme';
  if (!draft.model.trim()) return 'model';
  const preset = presetForEndpoint(endpoint);
  if (preset?.requiresKey && !draft.apiKey.trim()) return 'apiKey';
  return null;
}
