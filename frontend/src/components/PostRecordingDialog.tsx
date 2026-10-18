'use client';

import React, { useState } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { FileText, Sparkles, Mic, Loader2 } from 'lucide-react';
import {
  buildPostRecordingPrompt,
  type PostRecordingAction,
  type PostRecordingInput,
} from '@/lib/post-recording';
import { useI18n } from '@/contexts/I18nContext';

interface PostRecordingDialogProps {
  open: boolean;
  /** Shape of the recording that just finished. */
  input: PostRecordingInput;
  /** Called with the chosen action; resolves when the follow-up work is done. */
  onChoose: (action: PostRecordingAction) => Promise<void>;
  /** Called when the user dismisses without choosing (treated as "keep"). */
  onDismiss: () => void;
}

/**
 * Ask what to do with a recording that has just stopped.
 *
 * Previously this was decided for the user: with live transcription the meeting
 * was saved and a summary was generated only if a global toggle happened to be
 * on, and with deferred transcription nothing was saved at all. Asking at this
 * moment is the point where the user actually knows whether they want the text
 * and the summary.
 */
export function PostRecordingDialog({
  open,
  input,
  onChoose,
  onDismiss,
}: PostRecordingDialogProps) {
  const { t } = useI18n();
  const [busyAction, setBusyAction] = useState<PostRecordingAction | null>(null);
  const prompt = buildPostRecordingPrompt(input);

  if (!prompt.show) return null;

  // Prompt text comes from the pure module for decisions, but the labels live in
  // the string table so this dialog follows the interface language like the rest
  // of the app.
  const labelFor = (action: PostRecordingAction) => {
    switch (action) {
      case 'transcribe':
        return { label: t('postRecording.transcribe'), description: t('postRecording.transcribeDescription') };
      case 'transcribe-and-summarize':
        return {
          label: t('postRecording.transcribeAndSummarise'),
          description: t('postRecording.transcribeAndSummariseDescription'),
        };
      case 'summarize':
        return { label: t('postRecording.summarise'), description: t('postRecording.summariseDescription') };
      case 'keep':
        return { label: t('postRecording.keep'), description: t('postRecording.keepDescription') };
    }
  };

  const message = input.transcriptCount > 0
    ? t('postRecording.hasTranscriptMessage')
    : input.deferred
      ? t('postRecording.deferredMessage')
      : t('postRecording.noSpeechMessage');

  const busy = busyAction !== null;

  const handleChoose = async (action: PostRecordingAction) => {
    if (busy) return;
    setBusyAction(action);
    try {
      await onChoose(action);
    } finally {
      setBusyAction(null);
    }
  };

  const iconFor = (action: PostRecordingAction) => {
    switch (action) {
      case 'transcribe':
        return <FileText className="w-4 h-4" />;
      case 'transcribe-and-summarize':
        return <Sparkles className="w-4 h-4" />;
      case 'summarize':
        return <Sparkles className="w-4 h-4" />;
      case 'keep':
        return <Mic className="w-4 h-4" />;
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // Closing mid-work would leave a transcription running with no UI.
        if (!next && !busy) onDismiss();
      }}
    >
      <DialogContent
        className="max-w-md"
        onEscapeKeyDown={(event) => {
          if (busy) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (busy) event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle>{t('postRecording.title')}</DialogTitle>
          <DialogDescription>{message}</DialogDescription>
        </DialogHeader>

        <div className="space-y-2 mt-2">
          {prompt.choices.map((choice) => (
            <Button
              key={choice.action}
              variant={choice.action === 'keep' ? 'ghost' : 'outline'}
              className="w-full h-auto justify-start items-start gap-3 py-3 px-4 text-left whitespace-normal"
              onClick={() => handleChoose(choice.action)}
              disabled={busy}
            >
              <span className="mt-0.5 shrink-0">
                {busyAction === choice.action ? (
                  <Loader2 className="w-4 h-4 animate-spin" />
                ) : (
                  iconFor(choice.action)
                )}
              </span>
              <span className="flex flex-col gap-0.5">
                <span className="font-medium">{labelFor(choice.action).label}</span>
                <span className="text-xs text-gray-500 font-normal">
                  {labelFor(choice.action).description}
                </span>
              </span>
            </Button>
          ))}
        </div>

        {busy && (
          <p className="text-xs text-gray-500 mt-1">
            {busyAction === 'keep' ? t('postRecording.finishing') : t('postRecording.working')}
          </p>
        )}
      </DialogContent>
    </Dialog>
  );
}
