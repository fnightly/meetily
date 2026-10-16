'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Loader2, RotateCcw, SlidersHorizontal } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';

interface DiarizationPreferences {
  threshold: number;
  max_speakers: number;
}

interface DiarizationPreferenceView {
  preferences: DiarizationPreferences;
  bounds: {
    min_threshold: number;
    max_threshold: number;
    min_speakers: number;
    max_speakers: number;
    calibrated_threshold: number;
  };
  is_default: boolean;
}

/**
 * Tuning for speaker separation.
 *
 * Collapsed by default and described in terms of what the user sees rather than what the
 * numbers mean, because the correct action for almost everyone is to leave them alone. They
 * exist for the two cases where that is not true: the result has the wrong number of
 * speakers, or the embedding model has been replaced and the calibration no longer applies.
 *
 * The limits are rendered as text rather than enforced only by the input, so the user can
 * see why a value was refused.
 */
export function SpeakerSeparationTuning() {
  const { t } = useI18n();
  const [view, setView] = useState<DiarizationPreferenceView | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [open, setOpen] = useState(false);

  const [threshold, setThreshold] = useState('');
  const [maxSpeakers, setMaxSpeakers] = useState('');

  const apply = useCallback((next: DiarizationPreferenceView) => {
    setView(next);
    setThreshold(String(next.preferences.threshold));
    setMaxSpeakers(String(next.preferences.max_speakers));
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      apply(await invoke<DiarizationPreferenceView>('get_diarization_preferences'));
    } catch (error) {
      console.error('Failed to read speaker separation settings:', error);
      toast.error(t('speakers.tuningLoadFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setLoading(false);
    }
  }, [apply, t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = async () => {
    setSaving(true);
    try {
      // Parsed here rather than trusting the input: an empty or half-typed field would
      // otherwise be sent as NaN and clamped to a value the user did not choose.
      const parsedThreshold = Number.parseFloat(threshold);
      const parsedSpeakers = Number.parseInt(maxSpeakers, 10);

      if (!Number.isFinite(parsedThreshold) || !Number.isFinite(parsedSpeakers)) {
        toast.error(t('speakers.tuningInvalid'));
        return;
      }

      // The command returns what was stored, so a clamped value is shown as stored rather
      // than leaving the field displaying a number that was not accepted.
      const stored = await invoke<DiarizationPreferenceView>('set_diarization_preferences', {
        preferences: { threshold: parsedThreshold, max_speakers: parsedSpeakers },
      });

      apply(stored);
      toast.success(t('speakers.tuningSaved'));
    } catch (error) {
      console.error('Failed to save speaker separation settings:', error);
      toast.error(t('speakers.tuningSaveFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  const reset = async () => {
    setSaving(true);
    try {
      apply(await invoke<DiarizationPreferenceView>('reset_diarization_preferences'));
      toast.success(t('speakers.tuningReset'));
    } catch (error) {
      console.error('Failed to reset speaker separation settings:', error);
      toast.error(t('speakers.tuningSaveFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  if (!view) {
    // Keep the section on screen when the settings could not be read. Disappearing would
    // leave the user with only a toast and no way back to the controls.
    return (
      <div className="px-1">
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={loading}
          className="flex items-center gap-2 text-sm text-gray-700 hover:text-gray-900 disabled:opacity-60"
        >
          {loading ? (
            <Loader2 className="w-4 h-4 animate-spin" />
          ) : (
            <SlidersHorizontal className="w-4 h-4" />
          )}
          <span>{t('speakers.tuning')}</span>
          {!loading && (
            <span className="text-xs text-amber-700">{t('speakers.tuningUnavailable')}</span>
          )}
        </button>
      </div>
    );
  }

  const { bounds } = view;

  return (
    <div className="px-1">
      <button
        type="button"
        onClick={() => setOpen(current => !current)}
        className="flex items-center gap-2 text-sm text-gray-700 hover:text-gray-900"
      >
        <SlidersHorizontal className="w-4 h-4" />
        <span>{t('speakers.tuning')}</span>
        <span className="text-xs text-gray-400">{t('speakers.tuningHint')}</span>
      </button>

      {open && (
        <div className="mt-3 p-4 border rounded-lg space-y-4">
          {!view.is_default && (
            <p className="text-xs text-amber-700">{t('speakers.tuningChanged')}</p>
          )}

          <div className="space-y-1">
            <label className="flex items-center justify-between text-sm text-gray-700">
              <span>{t('speakers.threshold')}</span>
              <span className="text-xs text-gray-500">
                {t('speakers.thresholdRange', {
                  min: bounds.min_threshold,
                  max: bounds.max_threshold,
                })}
              </span>
            </label>
            <input
              type="number"
              step="0.01"
              min={bounds.min_threshold}
              max={bounds.max_threshold}
              value={threshold}
              onChange={event => setThreshold(event.target.value)}
              className="w-full rounded border border-gray-300 px-2 py-1 text-sm"
            />
            {/* Explain the direction, because the number alone does not say which way to move. */}
            <p className="text-xs text-gray-500">
              {t('speakers.thresholdHelp', { calibrated: bounds.calibrated_threshold })}
            </p>
          </div>

          <div className="space-y-1">
            <label className="flex items-center justify-between text-sm text-gray-700">
              <span>{t('speakers.maxSpeakers')}</span>
              <span className="text-xs text-gray-500">
                {t('speakers.maxSpeakersRange', {
                  min: bounds.min_speakers,
                  max: bounds.max_speakers,
                })}
              </span>
            </label>
            <input
              type="number"
              step="1"
              min={bounds.min_speakers}
              max={bounds.max_speakers}
              value={maxSpeakers}
              onChange={event => setMaxSpeakers(event.target.value)}
              className="w-full rounded border border-gray-300 px-2 py-1 text-sm"
            />
            <p className="text-xs text-gray-500">{t('speakers.maxSpeakersHelp')}</p>
          </div>

          <div className="flex items-center gap-2">
            <Button size="sm" onClick={() => void save()} disabled={saving}>
              {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : null}
              <span className="ml-2">{t('common.save')}</span>
            </Button>

            <Button variant="outline" size="sm" onClick={() => void reset()} disabled={saving}>
              <RotateCcw className="w-4 h-4" />
              <span className="ml-2">{t('speakers.tuningResetAction')}</span>
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
