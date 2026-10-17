import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { Label } from './ui/label';
import { CheckCircle2, ExternalLink, Eye, EyeOff, Loader2, ShieldCheck, CloudUpload } from 'lucide-react';
import { ModelManager } from './WhisperModelManager';
import { ParakeetModelManager } from './ParakeetModelManager';
import { useSpeechProviders, type SpeechProviderView } from '@/hooks/useSpeechProviders';
import { useI18n } from '@/contexts/I18nContext';
import { cn } from '@/lib/utils';

export interface TranscriptModelProps {
    provider: 'localWhisper' | 'parakeet' | 'deepgram' | 'elevenLabs' | 'groq' | 'openai' | 'siliconflow';
    model: string;
    apiKey?: string | null;
}

export interface TranscriptSettingsProps {
    transcriptModelConfig: TranscriptModelProps;
    setTranscriptModelConfig: (config: TranscriptModelProps) => void;
    onModelSelect?: () => void;
}

/** Used before the backend catalog arrives (and if it fails), so the panel is never blank. */
const FALLBACK_PROVIDERS: SpeechProviderView[] = [
    {
        id: 'parakeet', display_name: 'Parakeet', kind: 'local', status: 'available',
        supported_languages: ['en'], requires_api_key: false, has_api_key: false,
        models: [], notes: '', api_key_url: null, selectable: true,
    },
    {
        id: 'localWhisper', display_name: 'Local Whisper', kind: 'local', status: 'available',
        supported_languages: null, requires_api_key: false, has_api_key: false,
        models: [], notes: '', api_key_url: null, selectable: true,
    },
];

function isLocalId(id: string) {
    return id === 'localWhisper' || id === 'parakeet';
}

export function TranscriptSettings({ transcriptModelConfig, setTranscriptModelConfig, onModelSelect }: TranscriptSettingsProps) {
    const { t } = useI18n();
    const [uiProvider, setUiProvider] = useState<TranscriptModelProps['provider']>(transcriptModelConfig.provider);

    // Provider capabilities come from the backend catalog (single source of
    // truth); the UI must not hardcode which providers exist or what they support.
    const { providers: catalog, active, validateSelection, refresh } = useSpeechProviders();
    const providers = catalog && catalog.length > 0 ? catalog : FALLBACK_PROVIDERS;
    const selectedProvider = providers.find((p) => p.id === uiProvider);

    // Cloud draft state
    const [cloudModel, setCloudModel] = useState('');
    const [apiKey, setApiKey] = useState('');
    const [showApiKey, setShowApiKey] = useState(false);
    const [isTesting, setIsTesting] = useState(false);
    const [isSaving, setIsSaving] = useState(false);
    const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);

    // Surface a provider/language mismatch as soon as the user opens settings,
    // rather than letting the failure appear when they hit record.
    const [selectionWarning, setSelectionWarning] = useState<string | null>(null);
    useEffect(() => {
        let cancelled = false;
        const language = active?.language ?? null;
        if (!uiProvider) return;

        validateSelection(uiProvider, language).then((problem) => {
            if (!cancelled) setSelectionWarning(problem);
        });

        return () => {
            cancelled = true;
        };
    }, [uiProvider, active?.language, validateSelection]);

    // Sync uiProvider when backend config changes (e.g., after model selection or initial load)
    useEffect(() => {
        setUiProvider(transcriptModelConfig.provider);
    }, [transcriptModelConfig.provider]);

    // Reset the cloud draft whenever the selected cloud provider changes.
    useEffect(() => {
        setTestResult(null);
        setApiKey('');
        setShowApiKey(false);
        if (!selectedProvider || isLocalId(selectedProvider.id)) return;
        const savedModel =
            transcriptModelConfig.provider === selectedProvider.id ? transcriptModelConfig.model : '';
        setCloudModel(savedModel || selectedProvider.models[0] || '');
    }, [selectedProvider?.id]); // eslint-disable-line react-hooks/exhaustive-deps

    const noteFor = (p: SpeechProviderView) => {
        const key = `speech.note.${p.id}`;
        const translated = t(key);
        return translated === key ? p.notes : translated;
    };

    const handleWhisperModelSelect = (modelName: string) => {
        setTranscriptModelConfig({ ...transcriptModelConfig, provider: 'localWhisper', model: modelName });
        onModelSelect?.();
    };

    const handleParakeetModelSelect = (modelName: string) => {
        setTranscriptModelConfig({ ...transcriptModelConfig, provider: 'parakeet', model: modelName });
        onModelSelect?.();
    };

    const hasUsableKey = !!apiKey.trim() || !!selectedProvider?.has_api_key;

    const handleTest = async () => {
        if (!selectedProvider) return;
        setIsTesting(true);
        setTestResult(null);
        try {
            const label = await invoke<string>('api_test_speech_provider', {
                provider: selectedProvider.id,
                model: cloudModel || null,
                apiKey: apiKey.trim() || null,
            });
            setTestResult({ ok: true, message: t('speech.testOk', { target: label }) });
        } catch (e) {
            setTestResult({ ok: false, message: e instanceof Error ? e.message : String(e) });
        } finally {
            setIsTesting(false);
        }
    };

    const handleSaveCloud = async () => {
        if (!selectedProvider) return;
        if (!hasUsableKey) {
            toast.error(t('speech.needKey'));
            return;
        }
        setIsSaving(true);
        try {
            await invoke('api_save_transcript_config', {
                provider: selectedProvider.id,
                model: cloudModel,
                apiKey: apiKey.trim() || null,
            });
            setTranscriptModelConfig({
                provider: selectedProvider.id as TranscriptModelProps['provider'],
                model: cloudModel,
                apiKey: null,
            });
            setApiKey('');
            await refresh();
            toast.success(t('speech.saved', { name: selectedProvider.display_name }));
            onModelSelect?.();
        } catch (e) {
            toast.error(t('speech.saveFailed'), { description: e instanceof Error ? e.message : String(e) });
        } finally {
            setIsSaving(false);
        }
    };

    const groups: { kind: 'local' | 'cloud'; title: string; hint: string }[] = [
        { kind: 'local', title: t('speech.group.local'), hint: t('speech.group.localHint') },
        { kind: 'cloud', title: t('speech.group.cloud'), hint: t('speech.group.cloudHint') },
    ];

    const isActive = (p: SpeechProviderView) => transcriptModelConfig.provider === p.id;

    return (
        <div className="space-y-6 pb-6">
            {groups.map((group) => {
                // Usable providers first; reserved ones sink to the end of their group.
                const items = providers
                    .filter((p) => p.kind === group.kind)
                    .sort((a, b) => Number(b.selectable) - Number(a.selectable));
                if (items.length === 0) return null;
                return (
                    <section key={group.kind} aria-label={group.title}>
                        <div className="mb-2 flex items-baseline justify-between">
                            <h4 className="flex items-center gap-1.5 text-sm font-semibold text-gray-900">
                                {group.kind === 'local' ? (
                                    <ShieldCheck className="h-4 w-4 text-emerald-600" />
                                ) : (
                                    <CloudUpload className="h-4 w-4 text-sky-600" />
                                )}
                                {group.title}
                            </h4>
                            <span className="text-xs text-gray-500">{group.hint}</span>
                        </div>
                        <div role="radiogroup" aria-label={group.title} className="grid gap-2 sm:grid-cols-2">
                            {items.map((p) => {
                                const selected = uiProvider === p.id;
                                return (
                                    <button
                                        key={p.id}
                                        type="button"
                                        role="radio"
                                        aria-checked={selected}
                                        disabled={!p.selectable}
                                        onClick={() => setUiProvider(p.id as TranscriptModelProps['provider'])}
                                        className={cn(
                                            'rounded-lg border p-3 text-left transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500',
                                            selected ? 'border-blue-500 bg-blue-50/60' : 'border-gray-200 bg-white hover:bg-gray-50',
                                            !p.selectable && 'cursor-not-allowed opacity-50 hover:bg-white',
                                        )}
                                    >
                                        <div className="flex items-center justify-between gap-2">
                                            <span className="text-sm font-medium text-gray-900">{p.display_name}</span>
                                            <span className="flex shrink-0 gap-1">
                                                {isActive(p) && (
                                                    <span className="rounded bg-emerald-100 px-1.5 py-0.5 text-[10px] font-medium text-emerald-700">
                                                        {t('speech.badge.inUse')}
                                                    </span>
                                                )}
                                                {p.supported_languages?.length === 1 && p.supported_languages[0] === 'en' && (
                                                    <span className="rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-gray-600">
                                                        {t('speech.badge.englishOnly')}
                                                    </span>
                                                )}
                                                {p.kind === 'cloud' && p.selectable && p.has_api_key && (
                                                    <span className="rounded bg-sky-100 px-1.5 py-0.5 text-[10px] text-sky-700">
                                                        {t('speech.badge.keySaved')}
                                                    </span>
                                                )}
                                                {!p.selectable && (
                                                    <span className="rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-gray-500">
                                                        {t('speech.badge.comingSoon')}
                                                    </span>
                                                )}
                                            </span>
                                        </div>
                                        <p className="mt-1 line-clamp-2 text-xs text-gray-500">{noteFor(p)}</p>
                                    </button>
                                );
                            })}
                        </div>
                    </section>
                );
            })}

            {selectionWarning && (
                <p role="status" className="rounded border border-red-200 bg-red-50 p-2 text-xs text-red-700">
                    {selectionWarning}
                </p>
            )}

            {uiProvider === 'localWhisper' && (
                <ModelManager
                    selectedModel={transcriptModelConfig.provider === 'localWhisper' ? transcriptModelConfig.model : undefined}
                    onModelSelect={handleWhisperModelSelect}
                    autoSave={true}
                />
            )}

            {uiProvider === 'parakeet' && (
                <ParakeetModelManager
                    selectedModel={transcriptModelConfig.provider === 'parakeet' ? transcriptModelConfig.model : undefined}
                    onModelSelect={handleParakeetModelSelect}
                    autoSave={true}
                />
            )}

            {selectedProvider && selectedProvider.kind === 'cloud' && selectedProvider.selectable && (
                <section className="space-y-4 rounded-lg border border-gray-200 bg-white p-4" aria-label={selectedProvider.display_name}>
                    <div className="rounded-md border border-amber-200 bg-amber-50 p-3 text-xs text-amber-800">
                        {t('speech.cloudPrivacy', { name: selectedProvider.display_name })}
                    </div>

                    <div>
                        <Label htmlFor="cloud-stt-model" className="mb-1 block text-sm font-medium text-gray-700">
                            {t('speech.model')}
                        </Label>
                        <Select value={cloudModel} onValueChange={setCloudModel}>
                            <SelectTrigger id="cloud-stt-model">
                                <SelectValue placeholder={t('speech.selectModel')} />
                            </SelectTrigger>
                            <SelectContent>
                                {selectedProvider.models.map((model) => (
                                    <SelectItem key={model} value={model}>{model}</SelectItem>
                                ))}
                            </SelectContent>
                        </Select>
                    </div>

                    <div>
                        <div className="mb-1 flex items-center justify-between">
                            <Label htmlFor="cloud-stt-key" className="text-sm font-medium text-gray-700">API Key</Label>
                            {selectedProvider.api_key_url && (
                                <a
                                    href={selectedProvider.api_key_url}
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    className="inline-flex items-center gap-1 text-xs text-blue-600 hover:underline"
                                >
                                    {t('speech.getKey')} <ExternalLink className="h-3 w-3" />
                                </a>
                            )}
                        </div>
                        <div className="relative">
                            <Input
                                id="cloud-stt-key"
                                type={showApiKey ? 'text' : 'password'}
                                autoComplete="off"
                                className="pr-10"
                                value={apiKey}
                                onChange={(e) => setApiKey(e.target.value)}
                                placeholder={selectedProvider.has_api_key ? t('speech.keyStoredPlaceholder') : t('speech.keyPlaceholder')}
                            />
                            <Button
                                type="button"
                                variant="ghost"
                                size="icon"
                                className="absolute inset-y-0 right-0"
                                onClick={() => setShowApiKey(!showApiKey)}
                                aria-label={showApiKey ? t('speech.hideKey') : t('speech.showKey')}
                            >
                                {showApiKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                            </Button>
                        </div>
                    </div>

                    {testResult && (
                        <p
                            role="status"
                            className={cn(
                                'rounded border p-2 text-xs',
                                testResult.ok ? 'border-emerald-200 bg-emerald-50 text-emerald-800' : 'border-red-200 bg-red-50 text-red-700',
                            )}
                        >
                            {testResult.message}
                        </p>
                    )}

                    <div className="flex flex-wrap gap-2">
                        <Button type="button" variant="outline" onClick={handleTest} disabled={isTesting || !hasUsableKey || !cloudModel}>
                            {isTesting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <CheckCircle2 className="mr-2 h-4 w-4" />}
                            {isTesting ? t('speech.testing') : t('speech.test')}
                        </Button>
                        <Button type="button" onClick={handleSaveCloud} disabled={isSaving || !hasUsableKey || !cloudModel}>
                            {isSaving && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                            {isActive(selectedProvider) ? t('speech.saveChanges') : t('speech.useThis')}
                        </Button>
                    </div>
                    <p className="text-xs text-gray-500">{t('speech.cloudBatchHint')}</p>
                </section>
            )}
        </div>
    );
}
