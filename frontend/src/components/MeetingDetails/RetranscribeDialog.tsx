import React, { useState, useEffect, useRef, useMemo } from 'react';
import { useRouter } from 'next/navigation';
import { RefreshCw, Globe, Cpu, Settings2 } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { LANGUAGES } from '@/constants/languages';
import { useTranscriptionModels, ModelOption } from '@/hooks/useTranscriptionModels';
import { useI18n } from '@/contexts/I18nContext';
import { enqueueTranscription } from '@/lib/background-transcription';

interface RetranscribeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingFolderPath: string | null;
  /** Shown in the progress and completion messages. */
  meetingTitle?: string;
  /** The recording has no transcript yet: the dialog is a first transcription. */
  emptyTranscript?: boolean;
}

/**
 * Pick a language and model, then hand the recording to the background queue.
 *
 * The dialog used to stay open, and refuse to close, until the transcription
 * finished. It now closes as soon as the job is queued; progress shows in a
 * toast and on the transcript button, and the result is announced when done.
 */
export function RetranscribeDialog({
  open,
  onOpenChange,
  meetingId,
  meetingFolderPath,
  meetingTitle,
  emptyTranscript = false,
}: RetranscribeDialogProps) {
  const { t } = useI18n();
  const router = useRouter();
  const { selectedLanguage, transcriptModelConfig } = useConfig();
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');

  // Use centralized model fetching hook
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
    loadError,
    fetchModels,
    resetSelection,
  } = useTranscriptionModels(transcriptModelConfig);

  // Track previous open state to only reset on closed→open transition
  const prevOpenRef = useRef(false);

  // Helper to get selected model details (memoized)
  const selectedModelDetails = useMemo((): ModelOption | undefined => {
    if (!selectedModelKey) return undefined;
    const colonIndex = selectedModelKey.indexOf(':');
    if (colonIndex === -1) return undefined;
    const provider = selectedModelKey.slice(0, colonIndex);
    const name = selectedModelKey.slice(colonIndex + 1);
    return availableModels.find(m => m.provider === provider && m.name === name);
  }, [selectedModelKey, availableModels]);
  const isParakeetModel = selectedModelDetails?.provider === 'parakeet';

  useEffect(() => {
    if (isParakeetModel && selectedLang !== 'auto') {
      setSelectedLang('auto');
    }
  }, [isParakeetModel, selectedLang]);

  // Reset state only when dialog transitions from closed to open
  // This prevents re-initialization when config changes while dialog is already open
  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;

    if (open && !wasOpen) {
      resetSelection();
      setSelectedLang(selectedLanguage || 'auto');

      // Fetch available models using centralized hook
      fetchModels();
    }
  }, [open, selectedLanguage, transcriptModelConfig, fetchModels]);

  const handleStartRetranscription = () => {
    if (!meetingFolderPath) {
      toast.error(t('retranscribe.noFolder'));
      return;
    }

    enqueueTranscription([{
      folderPath: meetingFolderPath,
      meetingId,
      title: meetingTitle,
      model: selectedModelDetails,
      language: isParakeetModel || selectedLang === 'auto' ? null : selectedLang,
    }]);
    toast.success(t('bgTranscribe.started'), { description: t('bgTranscribe.startedDetail') });
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[450px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <RefreshCw className="h-5 w-5 text-blue-600" />
            {emptyTranscript
              ? t('transcriptActions.transcribeRecording')
              : t('retranscribe.title')}
          </DialogTitle>
          <DialogDescription>
            {emptyTranscript
              ? t('retranscribe.emptyDescription')
              : t('retranscribe.description')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {/* The Start button still works with the configured default model, but
              the empty list must not look like a broken picker. */}
          {!loadingModels && availableModels.length === 0 && (
            <div role="status" className="space-y-1.5 rounded-lg border border-amber-200 bg-amber-50 p-3">
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
          )}

          {!isParakeetModel ? (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Globe className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">{t('retranscribe.language')}</span>
              </div>
              <Select value={selectedLang} onValueChange={setSelectedLang}>
                <SelectTrigger className="w-full" aria-label={t('retranscribe.language')}>
                  <SelectValue placeholder={t('retranscribe.languagePlaceholder')} />
                </SelectTrigger>
                <SelectContent className="max-h-60">
                  {LANGUAGES.map((lang) => (
                    <SelectItem key={lang.code} value={lang.code}>
                      {lang.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">{t('retranscribe.languageHint')}</p>
            </div>
          ) : (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Globe className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">{t('retranscribe.language')}</span>
              </div>
              <p className="text-xs text-muted-foreground">{t('retranscribe.parakeetLanguage')}</p>
            </div>
          )}

          {availableModels.length > 0 && (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Cpu className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">{t('retranscribe.model')}</span>
              </div>
              <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels}>
                <SelectTrigger className="w-full" aria-label={t('retranscribe.model')}>
                  <SelectValue placeholder={loadingModels ? t('retranscribe.loadingModels') : t('retranscribe.selectModel')} />
                </SelectTrigger>
                <SelectContent>
                  {availableModels.map((model) => (
                    <SelectItem key={`${model.provider}:${model.name}`} value={`${model.provider}:${model.name}`} disabled={model.disabled}>
                      {model.displayName}{model.size_mb > 0 ? ` (${Math.round(model.size_mb)} MB)` : ""}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">{t('retranscribe.modelHint')}</p>
            </div>
          )}

        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          <Button
            onClick={handleStartRetranscription}
            className="bg-blue-600 hover:bg-blue-700"
            disabled={!meetingFolderPath}
          >
            <RefreshCw className="h-4 w-4 mr-2" />
            {t('retranscribe.start')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
