import React, { useState, useEffect, useRef } from 'react';
import { Switch } from '@/components/ui/switch';
import { FolderOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { DeviceSelection, SelectedDevices } from '@/components/DeviceSelection';
import Analytics from '@/lib/analytics';
import { toast } from 'sonner';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useConfig } from '@/contexts/ConfigContext';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Label } from '@/components/ui/label';
import { useI18n } from '@/contexts/I18nContext';
import { Button } from '@/components/ui/button';
import { formatDuration } from '@/hooks/useRecordingLevelMonitoring';
import { ModelResidencySettings, type ModelResidency } from '@/components/ModelResidencySettings';
import {
  DEFAULT_MICROPHONE_GAIN_DB,
  MAX_MICROPHONE_GAIN_DB,
  MIN_MICROPHONE_GAIN_DB,
  clampGainDb,
  formatGainDb,
  resolveCaptureSystemAudio,
  resolveMicrophoneGainDb,
  resolveNoiseSuppression,
} from '@/lib/mic-settings';

export interface RecordingPreferences {
  save_folder: string;
  auto_save: boolean;
  file_format: string;
  file_quality: string;
  defer_transcription: boolean;
  /** Rewrite the transcript with a local model after recording. Off by default. */
  cleanup_after_recording: boolean;
  /** Stop after this many seconds of silence. 0 disables. */
  silence_stop_enabled_seconds: number;
  /** Hard cap on recording length in seconds. 0 disables. */
  max_recording_seconds: number;
  /** Delete recordings older than this many days. 0 keeps them forever. */
  recordings_retention_days: number;
  /** Warn below this many MB free. 0 disables. */
  low_disk_warning_mb: number;
  /** How long the speech model stays loaded after use (speed vs memory). */
  model_residency?: ModelResidency;
  model_keep_warm_minutes?: number;
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
  /** Also capture the computer's own audio. False records the microphone only. */
  capture_system_audio?: boolean;
  /** Gain applied to the recorded microphone audio, in dB (-12..24). */
  microphone_gain_db?: number;
  /** Reduce steady background noise with RNNoise. */
  noise_suppression?: boolean;
}

/** Silence durations offered in the UI. */
const SILENCE_STOP_CHOICES = [60, 120, 300, 600];

/** Maximum recording lengths offered in the UI. */
const MAX_LENGTH_CHOICES = [1800, 3600, 7200, 14400];

/** Retention periods offered in the UI. */
const RETENTION_CHOICES = [7, 30, 90, 365];

/** One selectable format/quality, as reported by the backend. */
interface EncodingOption {
  id: string;
  label: string;
  lossless: boolean;
}

interface RecordingEncodingOptions {
  formats: EncodingOption[];
  qualities: EncodingOption[];
  default_format: string;
  default_quality: string;
}

interface RecordingSettingsProps {
  onSave?: (preferences: RecordingPreferences) => void;
}

export function RecordingSettings({ onSave }: RecordingSettingsProps) {
  const { t } = useI18n();
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    save_folder: '',
    auto_save: true,
    file_format: 'm4a',
    file_quality: 'medium',
    defer_transcription: false,
    cleanup_after_recording: false,
    silence_stop_enabled_seconds: 0,
    max_recording_seconds: 0,
    recordings_retention_days: 0,
    low_disk_warning_mb: 1024,
    model_residency: 'auto',
    model_keep_warm_minutes: 10,
    preferred_mic_device: null,
    preferred_system_device: null,
    capture_system_audio: true,
    microphone_gain_db: DEFAULT_MICROPHONE_GAIN_DB,
    noise_suppression: false
  });
  const [encodingOptions, setEncodingOptions] = useState<RecordingEncodingOptions | null>(null);
  const [purgePreview, setPurgePreview] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showRecordingNotification, setShowRecordingNotification] = useState(true);
  const { isRecording } = useRecordingState();
  const { setSelectedDevices, setSelectedWindow, setCaptureSystemAudio } = useConfig();
  // Last gain written to the backend, so releasing the slider (or blurring it)
  // does not fire a second invoke when nothing changed.
  const persistedGainRef = useRef(DEFAULT_MICROPHONE_GAIN_DB);

  // Format/quality lists come from the backend so the UI cannot offer a format
  // the encoder does not actually support.
  useEffect(() => {
    invoke<RecordingEncodingOptions>('get_recording_encoding_options')
      .then(setEncodingOptions)
      .catch((error) => console.error('Failed to load recording encoding options:', error));
  }, []);

  const selectedFormat = encodingOptions?.formats.find((f) => f.id === preferences.file_format);
  // Quality is a no-op for WAV/FLAC, so hide it rather than offering a dead control.
  const qualityApplies = selectedFormat ? !selectedFormat.lossless : true;

  // Load recording preferences on component mount
  useEffect(() => {
    const loadPreferences = async () => {
      try {
        const prefs = await invoke<RecordingPreferences>('get_recording_preferences');
        // Normalise the fields older backends omit so every save round-trips
        // concrete values instead of a mix of undefined and defaults.
        const normalized: RecordingPreferences = {
          ...prefs,
          capture_system_audio: resolveCaptureSystemAudio(prefs.capture_system_audio),
          microphone_gain_db: resolveMicrophoneGainDb(prefs.microphone_gain_db),
          noise_suppression: resolveNoiseSuppression(prefs.noise_suppression),
        };
        setPreferences(normalized);
        persistedGainRef.current = normalized.microphone_gain_db ?? DEFAULT_MICROPHONE_GAIN_DB;
      } catch (error) {
        console.error('Failed to load recording preferences:', error);
        // If loading fails, get default folder path
        try {
          const defaultPath = await invoke<string>('get_default_recordings_folder_path');
          setPreferences(prev => ({ ...prev, save_folder: defaultPath }));
        } catch (defaultError) {
          console.error('Failed to get default folder path:', defaultError);
        }
      } finally {
        setLoading(false);
      }
    };

    loadPreferences();
  }, []);

  // Load recording notification preference
  useEffect(() => {
    const loadNotificationPref = async () => {
      try {
        const { Store } = await import('@tauri-apps/plugin-store');
        const store = await Store.load('preferences.json');
        const show = await store.get<boolean>('show_recording_notification') ?? true;
        setShowRecordingNotification(show);
      } catch (error) {
        console.error('Failed to load notification preference:', error);
      }
    };
    loadNotificationPref();
  }, []);

  const handleAutoSaveToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, auto_save: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    // Track auto-save setting change
    await Analytics.track('auto_save_recording_toggled', {
      enabled: enabled.toString()
    });
  };

  const handleDeferTranscriptionToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, defer_transcription: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('defer_transcription_toggled', {
      enabled: enabled.toString()
    });
  };

  const handleFormatChange = async (format: string) => {
    const newPreferences = { ...preferences, file_format: format };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('recording_format_changed', { format });
  };

  const handleQualityChange = async (quality: string) => {
    const newPreferences = { ...preferences, file_quality: quality };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('recording_quality_changed', { quality });
  };

  const handleSilenceStopToggle = async (enabled: boolean) => {
    // Default to 5 minutes when first enabled: long enough that a considered
    // pause does not end the recording.
    const seconds = enabled ? preferences.silence_stop_enabled_seconds || 300 : 0;
    const newPreferences = { ...preferences, silence_stop_enabled_seconds: seconds };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('silence_stop_toggled', { enabled: String(enabled), seconds: String(seconds) });
  };

  const handleSilenceStopSecondsChange = async (seconds: number) => {
    const newPreferences = { ...preferences, silence_stop_enabled_seconds: seconds };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);
  };

  const handleMaxLengthToggle = async (enabled: boolean) => {
    // Default to two hours: beyond most meetings, but bounded.
    const seconds = enabled ? preferences.max_recording_seconds || 7200 : 0;
    const newPreferences = { ...preferences, max_recording_seconds: seconds };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('max_recording_length_toggled', { enabled: String(enabled), seconds: String(seconds) });
  };

  const handleMaxSecondsChange = async (seconds: number) => {
    const newPreferences = { ...preferences, max_recording_seconds: seconds };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);
  };

  const handleCleanupToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, cleanup_after_recording: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('transcript_cleanup_toggled', { enabled: String(enabled) });
  };

  const handleRetentionChange = async (days: number) => {    const newPreferences = { ...preferences, recordings_retention_days: days };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);
    // A stale preview would describe the previous policy.
    setPurgePreview(null);

    await Analytics.track('retention_changed', { days: String(days) });
  };

  /**
   * Show what retention would remove. Deliberately a preview rather than a
   * purge: deleting recordings is not something a settings page should do as a
   * side effect of being opened.
   */
  const handlePreviewPurge = async () => {
    setPreviewing(true);
    try {
      const result = await invoke<{ detail: string; deleted: string[] }>('preview_retention_sweep');
      const listed = result.deleted.length
        ? `\n${result.deleted.map((p) => `• ${p}`).join('\n')}`
        : '';
      setPurgePreview(`${result.detail}${listed}`);
    } catch (error) {
      console.error('Failed to preview retention sweep:', error);
      toast.error('Could not check which recordings would be deleted');
    } finally {
      setPreviewing(false);
    }
  };

  const handleDeviceChange = async (devices: SelectedDevices) => {
    const newPreferences = {
      ...preferences,
      preferred_mic_device: devices.micDevice,
      preferred_system_device: devices.systemDevice
    };
    setPreferences(newPreferences);
    // Sync the in-memory selection ConfigContext exposes to the start path.
    // Without this, the picker only writes to disk (loaded into context once at
    // app mount), so a newly-chosen mic isn't honored until the next launch —
    // start keeps sending the stale launch-time device.
    setSelectedDevices(devices);
    await savePreferences(newPreferences);

    // Track default device preference changes
    // Note: Individual device selection analytics are tracked in DeviceSelection component
    await Analytics.track('default_devices_changed', {
      has_preferred_microphone: (!!devices.micDevice).toString(),
      has_preferred_system_audio: (!!devices.systemDevice).toString()
    });
  };

  const handleCaptureSystemAudioChange = async (captureSystemAudio: boolean) => {
    const newPreferences = { ...preferences, capture_system_audio: captureSystemAudio };
    setPreferences(newPreferences);
    // The home screen reads the capture mode from the config context, which is
    // otherwise only loaded at startup; keep it in step with this change.
    setCaptureSystemAudio(captureSystemAudio);
    if (!captureSystemAudio) {
      // A window target only narrows system audio; with system audio off it would
      // keep the home screen pointing at a window that is never captured.
      setSelectedWindow(null);
    }
    await savePreferences(newPreferences);

    await Analytics.track('capture_system_audio_toggled', {
      enabled: captureSystemAudio.toString()
    });
  };

  // Dragging the slider only touches local state; the backend is written on
  // release so one drag is one invoke rather than one per pixel.
  const handleGainInput = (value: number) => {
    setPreferences(prev => ({ ...prev, microphone_gain_db: clampGainDb(value) }));
  };

  const commitGain = async (value: number) => {
    const gain = clampGainDb(value);
    if (gain === persistedGainRef.current) return;
    // Updated before the await so a blur immediately after pointerup is a no-op.
    persistedGainRef.current = gain;
    await savePreferences({ ...preferences, microphone_gain_db: gain });
  };

  const handleGainReset = async () => {
    const newPreferences = { ...preferences, microphone_gain_db: DEFAULT_MICROPHONE_GAIN_DB };
    setPreferences(newPreferences);
    persistedGainRef.current = DEFAULT_MICROPHONE_GAIN_DB;
    await savePreferences(newPreferences);

    await Analytics.track('microphone_gain_changed', {
      db: String(DEFAULT_MICROPHONE_GAIN_DB)
    });
  };

  const handleNoiseSuppressionToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, noise_suppression: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    await Analytics.track('noise_suppression_toggled', {
      enabled: enabled.toString()
    });
  };

  const handleOpenFolder = async () => {
    try {
      await invoke('open_recordings_folder');
    } catch (error) {
      console.error('Failed to open recordings folder:', error);
    }
  };

  const handleChangeFolder = async () => {
    try {
      const picked = await invoke<string | null>('select_recording_folder');
      if (!picked) return; // cancelled
      setPreferences(prev => ({ ...prev, save_folder: picked }));
      toast.success('Recordings folder updated', { description: picked });
      await Analytics.track('recordings_folder_changed', { changed: 'true' });
    } catch (error) {
      console.error('Failed to change recordings folder:', error);
      toast.error('Failed to change recordings folder', {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleNotificationToggle = async (enabled: boolean) => {
    try {
      setShowRecordingNotification(enabled);
      const { Store } = await import('@tauri-apps/plugin-store');
      const store = await Store.load('preferences.json');
      await store.set('show_recording_notification', enabled);
      await store.save();
      toast.success('Preference saved');
      await Analytics.track('recording_notification_preference_changed', {
        enabled: enabled.toString()
      });
    } catch (error) {
      console.error('Failed to save notification preference:', error);
      toast.error('Failed to save preference');
    }
  };

  const savePreferences = async (prefs: RecordingPreferences) => {
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: prefs });
      onSave?.(prefs);

      toast.success(t('recordingSettings.saved'));
    } catch (error) {
      console.error('Failed to save recording preferences:', error);
      toast.error(t('recordingSettings.saveFailed'), {
        description: error instanceof Error ? error.message : String(error)
      });
    } finally {
      setSaving(false);
    }
  };

  // Backend reads these when a recording starts, so they stay fixed while one runs.
  const controlsLocked = saving || isRecording;
  const captureSystemAudio = resolveCaptureSystemAudio(preferences.capture_system_audio);
  const gainDb = resolveMicrophoneGainDb(preferences.microphone_gain_db);

  if (loading) {
    return (
      <div className="animate-pulse">
        <div className="h-4 bg-gray-200 rounded w-1/4 mb-4"></div>
        <div className="h-8 bg-gray-200 rounded mb-4"></div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Auto Save Toggle */}
      <div className="flex items-center justify-between surface-card p-5">
        <div className="flex-1">
          <div className="font-medium">{t('recordingSettings.autoSave')}</div>
          <div className="text-sm text-gray-600">
            {t('recordingSettings.autoSaveHint')}
          </div>
        </div>
        <Switch
          checked={preferences.auto_save}
          onCheckedChange={handleAutoSaveToggle}
          disabled={saving}
        />
      </div>

      {/* Deferred Transcription Toggle */}
      <div className="flex items-center justify-between surface-card p-5">
        <div className="flex-1 pr-4">
          <div className="font-medium">{t('recordingSettings.defer')}</div>
          <div className="text-sm text-gray-600">
            {t('recordingSettings.deferHint')}
          </div>
          {preferences.defer_transcription && !preferences.auto_save && (
            <div className="text-xs text-amber-700 mt-2">
              {t('recordingSettings.deferNeedsSave')}
            </div>
          )}
        </div>
        <Switch
          checked={preferences.defer_transcription}
          onCheckedChange={handleDeferTranscriptionToggle}
          disabled={saving}
        />
      </div>

      {/* Speed vs memory: speech model residency */}
      <ModelResidencySettings
        value={preferences.model_residency ?? 'auto'}
        minutes={preferences.model_keep_warm_minutes ?? 10}
        disabled={saving}
        onChange={(value, minutes) => {
          const next = { ...preferences, model_residency: value, model_keep_warm_minutes: minutes };
          setPreferences(next);
          void savePreferences(next);
          void Analytics.track('model_residency_changed', { value, minutes: String(minutes) });
        }}
      />

      {/* Folder Location - Only shown when auto_save is enabled */}
      {preferences.auto_save && (
        <div className="space-y-4">
          <div className="surface-card p-5">
            <div className="font-semibold text-slate-900 mb-1">{t('recSet.saveLocation')}</div>
            <div className="text-xs text-slate-500 mb-3 break-all font-mono">
              {preferences.save_folder || t('recSet.defaultFolder')}
            </div>
            <div className="flex flex-wrap gap-2">
            <button
              onClick={handleOpenFolder}
              className="flex items-center gap-2 px-3 h-8 text-sm border border-slate-300 bg-white rounded-lg hover:bg-slate-50 transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              {t('storage.open')}
            </button>
            <button
              onClick={handleChangeFolder}
              disabled={saving || isRecording}
              className="flex items-center gap-2 px-3 h-8 text-sm border border-slate-300 bg-white rounded-lg hover:bg-slate-50 transition-colors disabled:opacity-50"
            >
              {t('storage.change')}
            </button>
            </div>
            <div className="text-xs text-slate-400 mt-2">
              {t('recSet.alsoInGeneral')}
            </div>
          </div>

          {/* Output format and quality. These were previously display-only: the
              format was saved but the encoder always wrote AAC/MP4. */}
          <div className="surface-card p-5 space-y-4">
            <div className="space-y-2">
              <Label htmlFor="rec-format">{t('recording.recordingFormat')}</Label>
              <Select
                value={preferences.file_format}
                onValueChange={handleFormatChange}
                disabled={saving || isRecording}
              >
                <SelectTrigger id="rec-format">
                  <SelectValue placeholder={t('recording.selectFormat')} />
                </SelectTrigger>
                <SelectContent>
                  {(encodingOptions?.formats ?? []).map((option) => (
                    <SelectItem key={option.id} value={option.id}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-gray-500">
                {t('recSet.savedAs', { file: `audio.${preferences.file_format}` })}
              </p>
            </div>

            {qualityApplies ? (
              <div className="space-y-2">
                <Label htmlFor="rec-quality">{t('recording.audioQuality')}</Label>
                <Select
                  value={preferences.file_quality}
                  onValueChange={handleQualityChange}
                  disabled={saving || isRecording}
                >
                  <SelectTrigger id="rec-quality">
                    <SelectValue placeholder={t('recording.selectQuality')} />
                  </SelectTrigger>
                  <SelectContent>
                    {(encodingOptions?.qualities ?? []).map((option) => (
                      <SelectItem key={option.id} value={option.id}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="text-xs text-gray-500">
                  {t('recording.qualityHint')}

                </p>
              </div>
            ) : (
              <p className="text-xs text-gray-500">
                {t('recording.losslessNote', { format: selectedFormat?.label ?? '' })}

              </p>
            )}

            {isRecording && (
              <p className="text-xs text-amber-700">
                {t('recording.lockedWhileRecording')}

              </p>
            )}
          </div>
        </div>
      )}

      {/* Info when auto_save is disabled */}
      {!preferences.auto_save && (
        <div className="p-4 border border-amber-200 rounded-xl bg-amber-50">
          <div className="text-sm text-amber-800">
            {t('recSet.disabled')}
          </div>
        </div>
      )}

      {/* Transcript cleanup. Placed above the limits because it changes what the
          saved transcript contains, which is more consequential than a guard. */}
      <div className="border-t pt-6">
        <h4 className="text-base font-medium text-gray-900 mb-1">{t('recording.cleanupTitle')}</h4>
        <div className="surface-card p-5 space-y-3">
          <div className="flex items-center justify-between gap-4">
            <Label htmlFor="rec-cleanup">{t('recording.cleanupToggle')}</Label>
            <Switch
              id="rec-cleanup"
              checked={preferences.cleanup_after_recording}
              onCheckedChange={handleCleanupToggle}
              disabled={saving || isRecording}
            />
          </div>
          <p className="text-sm text-gray-600">{t('recording.cleanupDescription')}</p>
          <p className="text-xs text-gray-500">{t('recording.cleanupLocalOnly')}</p>
        </div>
      </div>

      {/* Recording limits and retention. Defaults leave everything off, so the
          behaviour is unchanged until the user opts in. */}
      <div className="space-y-4">
        <div className="border-t pt-6">
          <h4 className="text-base font-medium text-gray-900 mb-1">{t('recording.limitsTitle')}</h4>
          <p className="text-sm text-gray-600 mb-4">
            {t('recording.limitsDescription')}
          </p>

          <div className="surface-card p-5 space-y-4">
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label htmlFor="rec-silence-stop">{t('recording.silenceStop')}</Label>
                <Switch
                  id="rec-silence-stop"
                  checked={preferences.silence_stop_enabled_seconds > 0}
                  onCheckedChange={handleSilenceStopToggle}
                  disabled={saving || isRecording}
                />
              </div>
              {preferences.silence_stop_enabled_seconds > 0 && (
                <Select
                  value={String(preferences.silence_stop_enabled_seconds)}
                  onValueChange={(value) => handleSilenceStopSecondsChange(Number(value))}
                  disabled={saving || isRecording}
                >
                  <SelectTrigger id="rec-silence-seconds">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {SILENCE_STOP_CHOICES.map((seconds) => (
                      <SelectItem key={seconds} value={String(seconds)}>
                        {t('recording.silenceOf', { duration: formatDuration(seconds) })}

                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              <p className="text-xs text-gray-500">
                {t('recording.silenceStopHint')}
              </p>
            </div>

            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label htmlFor="rec-max-length">{t('recording.maxLength')}</Label>
                <Switch
                  id="rec-max-length"
                  checked={preferences.max_recording_seconds > 0}
                  onCheckedChange={handleMaxLengthToggle}
                  disabled={saving || isRecording}
                />
              </div>
              {preferences.max_recording_seconds > 0 && (
                <Select
                  value={String(preferences.max_recording_seconds)}
                  onValueChange={(value) => handleMaxSecondsChange(Number(value))}
                  disabled={saving || isRecording}
                >
                  <SelectTrigger id="rec-max-seconds">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {MAX_LENGTH_CHOICES.map((seconds) => (
                      <SelectItem key={seconds} value={String(seconds)}>
                        {formatDuration(seconds)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              <p className="text-xs text-gray-500">
                {t('recording.maxLengthHint')}
              </p>
            </div>
          </div>
        </div>

        <div className="border-t pt-6">
          <h4 className="text-base font-medium text-gray-900 mb-1">{t('recording.retentionTitle')}</h4>
          <p className="text-sm text-gray-600 mb-4">
            {t('recording.retentionDescription')}
          </p>

          <div className="surface-card p-5 space-y-3">
            <div className="flex items-center gap-3">
              <Label htmlFor="rec-retention" className="shrink-0">
                {t('recording.keepFor')}
              </Label>
              <Select
                value={String(preferences.recordings_retention_days)}
                onValueChange={(value) => handleRetentionChange(Number(value))}
                disabled={saving || isRecording}
              >
                <SelectTrigger id="rec-retention">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="0">{t('recording.forever')}</SelectItem>
                  {RETENTION_CHOICES.map((days) => (
                    <SelectItem key={days} value={String(days)}>
                      {days} days
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {preferences.recordings_retention_days > 0 && (
              <div className="flex items-start gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handlePreviewPurge}
                  disabled={saving || isRecording || previewing}
                >
                  {previewing ? t('recording.checking') : t('recording.previewPurge')}

                </Button>
              </div>
            )}

            {purgePreview && (
              <div className="text-xs text-gray-600 bg-gray-50 border rounded p-2">
                {purgePreview}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Recording Notification Toggle */}
      <div className="flex items-center justify-between surface-card p-5">
        <div className="flex-1">
          <div className="font-medium">Recording Start Notification</div>
          <div className="text-sm text-gray-600">
            Show reminder to inform participants when recording starts
          </div>
        </div>
        <Switch
          checked={showRecordingNotification}
          onCheckedChange={handleNotificationToggle}
        />
      </div>

      {/* Recording source: what is captured, and how the microphone is treated.
          The backend reads these when a recording starts, so they are locked
          while one is in progress. */}
      <div className="space-y-4">
        <div className="border-t pt-6">
          <h4 className="text-base font-medium text-gray-900 mb-1">{t('recording.sourceTitle')}</h4>
          <p className="text-sm text-gray-600 mb-4">
            {t('recording.sourceDescription')}
          </p>

          {isRecording && (
            <p
              role="status"
              aria-live="polite"
              className="text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded-md p-2 mb-4"
            >
              {t('recording.lockedWhileRecording')}
            </p>
          )}

          <div className="surface-card p-5 space-y-5">
            {/* Two visible options read better than a switch whose label flips. */}
            <fieldset className="space-y-2" disabled={controlsLocked}>
              <legend className="sr-only">{t('recording.sourceTitle')}</legend>
              <label
                className={`flex cursor-pointer items-start gap-2.5 rounded-lg border p-3 transition-colors ${
                  captureSystemAudio ? 'border-blue-300 bg-blue-50/50' : 'border-slate-200 hover:bg-slate-50'
                } ${controlsLocked ? 'cursor-not-allowed opacity-60' : ''}`}
              >
                <input
                  type="radio"
                  name="rec-capture-source"
                  className="mt-0.5 h-4 w-4 accent-blue-600"
                  checked={captureSystemAudio}
                  onChange={() => void handleCaptureSystemAudioChange(true)}
                />
                <span className="text-sm text-slate-800">{t('recording.systemAndMic')}</span>
              </label>
              <label
                className={`flex cursor-pointer items-start gap-2.5 rounded-lg border p-3 transition-colors ${
                  !captureSystemAudio ? 'border-blue-300 bg-blue-50/50' : 'border-slate-200 hover:bg-slate-50'
                } ${controlsLocked ? 'cursor-not-allowed opacity-60' : ''}`}
              >
                <input
                  type="radio"
                  name="rec-capture-source"
                  className="mt-0.5 h-4 w-4 accent-blue-600"
                  checked={!captureSystemAudio}
                  onChange={() => void handleCaptureSystemAudioChange(false)}
                />
                <span className="flex min-w-0 flex-col">
                  <span className="text-sm text-slate-800">{t('recording.micOnly')}</span>
                  <span className="mt-0.5 text-xs text-slate-500">{t('recording.micOnlyHint')}</span>
                </span>
              </label>
            </fieldset>

            <div className="space-y-2">
              <div className="flex items-center justify-between gap-3">
                <Label htmlFor="rec-mic-gain">{t('recording.gainTitle')}</Label>
                <span className="text-sm font-medium tabular-nums text-slate-700">
                  {t('recording.gainValue', { db: formatGainDb(gainDb) })}
                </span>
              </div>
              <div className="flex items-center gap-3">
                <input
                  id="rec-mic-gain"
                  type="range"
                  min={MIN_MICROPHONE_GAIN_DB}
                  max={MAX_MICROPHONE_GAIN_DB}
                  step={1}
                  value={gainDb}
                  onChange={(event) => handleGainInput(Number(event.target.value))}
                  onPointerUp={(event) => void commitGain(Number(event.currentTarget.value))}
                  onKeyUp={(event) => void commitGain(Number(event.currentTarget.value))}
                  onBlur={(event) => void commitGain(Number(event.currentTarget.value))}
                  disabled={controlsLocked}
                  aria-valuetext={t('recording.gainValue', { db: formatGainDb(gainDb) })}
                  className="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-slate-200 accent-blue-600 disabled:cursor-not-allowed disabled:opacity-50"
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleGainReset()}
                  disabled={controlsLocked || gainDb === DEFAULT_MICROPHONE_GAIN_DB}
                >
                  {t('recording.gainReset')}
                </Button>
              </div>
              <p className="text-xs text-slate-500">{t('recording.gainHint')}</p>
            </div>

            <div className="space-y-1">
              <div className="flex items-center justify-between gap-4">
                <Label htmlFor="rec-noise-suppression">{t('recording.noiseSuppression')}</Label>
                <Switch
                  id="rec-noise-suppression"
                  checked={preferences.noise_suppression === true}
                  onCheckedChange={handleNoiseSuppressionToggle}
                  disabled={controlsLocked}
                />
              </div>
              <p className="text-xs text-slate-500">{t('recording.noiseSuppressionHint')}</p>
            </div>
          </div>
        </div>
      </div>

      {/* Device Preferences */}
      <div className="space-y-4">
        <div className="border-t pt-6">
          <h4 className="text-base font-medium text-gray-900 mb-4">Default Audio Devices</h4>
          <p className="text-sm text-gray-600 mb-4">
            Set your preferred microphone and system audio devices for recording. These will be automatically selected when starting new recordings.
          </p>

          {isRecording && (
            <p
              role="status"
              aria-live="polite"
              className="text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded-md p-2 mb-4"
            >
              Device selection is locked while a recording is in progress. Connecting a new device mid-recording will not switch to it. Stop the current meeting to change devices.
            </p>
          )}

          <div className="border rounded-lg p-4 bg-gray-50">
            <DeviceSelection
              selectedDevices={{
                micDevice: preferences.preferred_mic_device,
                systemDevice: preferences.preferred_system_device
              }}
              onDeviceChange={handleDeviceChange}
              disabled={saving || isRecording}
            />
          </div>
        </div>
      </div>
    </div>
  );
}