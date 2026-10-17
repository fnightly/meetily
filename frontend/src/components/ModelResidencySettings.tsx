'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Cpu, Gauge, Loader2, MemoryStick, Zap } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';

export type ModelResidency = 'auto' | 'onDemand' | 'keepWarm' | 'always';

export const RESIDENCY_OPTIONS: ModelResidency[] = ['auto', 'onDemand', 'keepWarm', 'always'];
const KEEP_WARM_CHOICES = [5, 10, 30, 60];

interface ResidencyStatus {
  preference: string;
  effective: 'onDemand' | 'keepWarm' | 'always';
  keep_warm_minutes: number;
  total_memory_gb: number | null;
  whisper_model: string | null;
  parakeet_model: string | null;
  idle_seconds: number | null;
}

interface ModelResidencySettingsProps {
  value: ModelResidency;
  minutes: number;
  disabled?: boolean;
  onChange: (value: ModelResidency, minutes: number) => void;
}

const ICONS: Record<ModelResidency, React.ReactNode> = {
  auto: <Gauge className="w-4 h-4" />,
  onDemand: <MemoryStick className="w-4 h-4" />,
  keepWarm: <Cpu className="w-4 h-4" />,
  always: <Zap className="w-4 h-4" />,
};

/**
 * Speed vs memory: how long the speech model stays loaded between recordings.
 * Shows what is resident right now and lets the user free it.
 */
export function ModelResidencySettings({ value, minutes, disabled, onChange }: ModelResidencySettingsProps) {
  const { t } = useI18n();
  const [status, setStatus] = useState<ResidencyStatus | null>(null);
  const [busy, setBusy] = useState<'release' | 'preload' | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await invoke<ResidencyStatus>('get_model_residency_status'));
    } catch (error) {
      console.error('Failed to read model residency status:', error);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15000);
    return () => window.clearInterval(timer);
  }, [refresh, value, minutes]);

  const resident = [status?.whisper_model && `Whisper ${status.whisper_model}`, status?.parakeet_model && `Parakeet ${status.parakeet_model}`]
    .filter(Boolean)
    .join(' · ');

  const release = async () => {
    setBusy('release');
    try {
      const released = await invoke<string[]>('release_speech_models');
      toast.success(released.length > 0 ? t('residency.released') : t('residency.nothingToRelease'));
    } catch (error) {
      toast.error(t('residency.releaseFailed'), { description: String(error) });
    } finally {
      setBusy(null);
      void refresh();
    }
  };

  const preload = async () => {
    setBusy('preload');
    try {
      const outcome = await invoke<{ status: string; detail: string | null }>('transcription_prewarm');
      if (outcome.status === 'loaded' || outcome.status === 'alreadyLoaded') {
        toast.success(t('residency.preloaded'), { description: outcome.detail ?? undefined });
      } else {
        toast.info(t('residency.preloadSkipped'), { description: outcome.detail ?? undefined });
      }
    } catch (error) {
      toast.error(t('residency.preloadSkipped'), { description: String(error) });
    } finally {
      setBusy(null);
      void refresh();
    }
  };

  return (
    <div className="surface-card p-5 space-y-3">
      <div>
        <div className="font-medium">{t('residency.title')}</div>
        <div className="text-sm text-gray-600">{t('residency.description')}</div>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-2" role="radiogroup" aria-label={t('residency.title')}>
        {RESIDENCY_OPTIONS.map((option) => (
          <button
            key={option}
            type="button"
            role="radio"
            aria-checked={value === option}
            disabled={disabled}
            onClick={() => onChange(option, minutes)}
            className={`text-left rounded-lg border px-3 py-2 transition-colors disabled:opacity-60 ${
              value === option ? 'border-blue-500 bg-blue-50 ring-1 ring-blue-500' : 'border-gray-200 hover:bg-gray-50'
            }`}
          >
            <div className="flex items-center gap-1.5 text-sm font-medium text-gray-900">
              {ICONS[option]}
              {t(`residency.opt.${option}`)}
              {option === 'auto' && (
                <span className="ml-auto text-[10px] rounded bg-green-100 text-green-800 px-1.5 py-0.5">
                  {t('residency.recommended')}
                </span>
              )}
            </div>
            <div className="text-xs text-gray-500 mt-0.5">{t(`residency.hint.${option}`)}</div>
          </button>
        ))}
      </div>

      {(value === 'keepWarm' || value === 'auto') && (
        <label className="flex items-center gap-2 text-sm text-gray-700">
          {t('residency.keepFor')}
          <select
            className="h-8 rounded-md border border-gray-200 bg-white px-2 text-sm"
            value={minutes || 10}
            disabled={disabled}
            onChange={(e) => onChange(value, Number(e.target.value))}
          >
            {KEEP_WARM_CHOICES.map((m) => (
              <option key={m} value={m}>
                {t('residency.minutes', { m })}
              </option>
            ))}
          </select>
        </label>
      )}

      <div className="flex items-center justify-between gap-3 rounded-md bg-gray-50 px-3 py-2 text-xs text-gray-600">
        <div className="min-w-0">
          {status ? (
            <>
              <div>
                {t('residency.effective')}: <strong>{t(`residency.opt.${status.effective}`)}</strong>
                {status.total_memory_gb ? ` · ${t('residency.ram', { gb: status.total_memory_gb })}` : ''}
              </div>
              <div className="truncate">
                {t('residency.resident')}: {resident || t('residency.none')}
              </div>
            </>
          ) : (
            <span>—</span>
          )}
        </div>
        <div className="flex gap-1.5 shrink-0">
          <Button size="sm" variant="outline" onClick={() => void preload()} disabled={busy !== null}>
            {busy === 'preload' ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Zap className="w-3.5 h-3.5" />}
            <span className="ml-1">{t('residency.preload')}</span>
          </Button>
          <Button size="sm" variant="outline" onClick={() => void release()} disabled={busy !== null || !resident}>
            {busy === 'release' ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <MemoryStick className="w-3.5 h-3.5" />}
            <span className="ml-1">{t('residency.release')}</span>
          </Button>
        </div>
      </div>
    </div>
  );
}
