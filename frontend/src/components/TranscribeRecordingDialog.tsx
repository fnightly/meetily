'use client';

import React, { useEffect, useMemo, useRef } from 'react';
import { useRouter } from 'next/navigation';
import { Cpu, Settings2 } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { useConfig } from '@/contexts/ConfigContext';
import { useI18n } from '@/contexts/I18nContext';
import { useTranscriptionModels } from '@/hooks/useTranscriptionModels';
import { modelKey, type ModelOption } from '@/lib/transcription-model-options';

interface TranscribeRecordingDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** Optional warning, e.g. that an existing transcript will be replaced. */
  body?: string;
  confirmLabel: string;
  destructive?: boolean;
  onConfirm: (model: ModelOption) => void;
}

/**
 * Model picker shown before the recordings library transcribes audio, so a
 * saved recording can use any local or cloud model rather than only the one
 * set in Settings.
 */
export function TranscribeRecordingDialog({
  open,
  onOpenChange,
  title,
  body,
  confirmLabel,
  destructive = false,
  onConfirm,
}: TranscribeRecordingDialogProps) {
  const { t } = useI18n();
  const router = useRouter();
  const { transcriptModelConfig } = useConfig();
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
    loadError,
    fetchModels,
    resetSelection,
  } = useTranscriptionModels(transcriptModelConfig);

  const prevOpenRef = useRef(false);
  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;
    if (open && !wasOpen) {
      resetSelection();
      fetchModels();
    }
  }, [open, fetchModels, resetSelection]);

  const selectedModel = useMemo(
    () => availableModels.find((m) => modelKey(m) === selectedModelKey && !m.disabled),
    [availableModels, selectedModelKey],
  );

  // No models were found (or every source failed): there is nothing to pick and
  // the confirm button would sit disabled forever without an explanation.
  const unavailable = !loadingModels && availableModels.length === 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{body ?? t('transcribeModels.description')}</DialogDescription>
        </DialogHeader>
        {unavailable ? (
          <div role="alert" className="space-y-2 rounded-lg border border-amber-200 bg-amber-50 p-3">
            <p className="text-sm font-medium text-amber-900">
              {loadError ? t('transcribeModels.loadFailed') : t('transcribeModels.noModels')}
            </p>
            <p className="text-xs leading-relaxed text-amber-800">
              {loadError ? t('transcribeModels.loadFailedHint') : t('transcribeModels.noModelsHint')}
            </p>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                onOpenChange(false);
                router.push('/settings');
              }}
            >
              <Settings2 className="mr-2 h-4 w-4" aria-hidden />
              {t('common.settings')}
            </Button>
          </div>
        ) : (
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <Cpu className="h-4 w-4 text-muted-foreground" aria-hidden />
              <span className="text-sm font-medium">{t('retranscribe.model')}</span>
            </div>
            <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels}>
              <SelectTrigger className="w-full" aria-label={t('retranscribe.model')}>
                <SelectValue
                  placeholder={loadingModels ? t('retranscribe.loadingModels') : t('retranscribe.selectModel')}
                />
              </SelectTrigger>
              <SelectContent>
                {availableModels.map((model) => (
                  <SelectItem key={modelKey(model)} value={modelKey(model)} disabled={model.disabled}>
                    {model.displayName}{model.size_mb > 0 ? ` (${Math.round(model.size_mb)} MB)` : ''}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">{t('retranscribe.modelHint')}</p>
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          <Button
            variant={destructive ? 'destructive' : 'default'}
            disabled={!selectedModel}
            title={!selectedModel ? t('transcribeModels.selectFirst') : undefined}
            onClick={() => selectedModel && onConfirm(selectedModel)}
          >
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
