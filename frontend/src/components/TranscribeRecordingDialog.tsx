'use client';

import React, { useEffect, useMemo, useRef } from 'react';
import { Cpu } from 'lucide-react';
import {
  Dialog,
  DialogContent,
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
  const { transcriptModelConfig } = useConfig();
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        {body && <p className="text-sm text-gray-600">{body}</p>}
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Cpu className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm font-medium">{t('retranscribe.model')}</span>
          </div>
          <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels}>
            <SelectTrigger className="w-full">
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
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          <Button
            variant={destructive ? 'destructive' : 'default'}
            disabled={!selectedModel}
            onClick={() => selectedModel && onConfirm(selectedModel)}
          >
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
