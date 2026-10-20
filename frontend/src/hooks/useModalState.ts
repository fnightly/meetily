import { useState, useEffect, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import type { TranscriptionErrorPayload } from '@/services/transcriptService';
import { tr } from '@/lib/i18n';

/** Payload of the backend's `transcript-chunk-loss-detected` event. */
interface ChunkLossPayload {
  chunks_queued?: number;
  chunks_completed?: number;
  chunks_lost?: number;
  message?: string;
}

export type ModalType =
  | 'modelSettings'
  | 'deviceSettings'
  | 'languageSettings'
  | 'modelSelector'
  | 'errorAlert'
  | 'chunkDropWarning';

interface ModalState {
  modelSettings: boolean;
  deviceSettings: boolean;
  languageSettings: boolean;
  modelSelector: boolean;
  errorAlert: boolean;
  chunkDropWarning: boolean;
}

interface ModalMessages {
  errorAlert: string;
  chunkDropWarning: string;
  modelSelector: string;
}

interface UseModalStateReturn {
  modals: ModalState;
  messages: ModalMessages;
  showModal: (name: ModalType, message?: string) => void;
  hideModal: (name: ModalType) => void;
  hideAllModals: () => void;
}

/**
 * Custom hook for managing all modal state and event listeners.
 * Consolidates 9 useState calls and 3 event listeners from page.tsx.
 *
 * Features:
 * - Unified modal state management
 * - Event listeners for chunk drops, transcription errors, model downloads
 * - Auto-close on model download completion
 */
export function useModalState(transcriptModelConfig?: TranscriptModelProps): UseModalStateReturn {
  // Modal visibility state
  const [modals, setModals] = useState<ModalState>({
    modelSettings: false,
    deviceSettings: false,
    languageSettings: false,
    modelSelector: false,
    errorAlert: false,
    chunkDropWarning: false,
  });

  // Modal messages
  const [messages, setMessages] = useState<ModalMessages>({
    errorAlert: '',
    chunkDropWarning: '',
    modelSelector: '',
  });

  // Show modal with optional message
  const showModal = useCallback((name: ModalType, message?: string) => {
    setModals(prev => ({ ...prev, [name]: true }));

    // Set message if provided
    if (message && (name === 'errorAlert' || name === 'chunkDropWarning' || name === 'modelSelector')) {
      setMessages(prev => ({ ...prev, [name]: message }));
    }
  }, []);

  // Hide modal and clear its message
  const hideModal = useCallback((name: ModalType) => {
    setModals(prev => ({ ...prev, [name]: false }));

    // Clear message when closing
    if (name === 'errorAlert' || name === 'chunkDropWarning' || name === 'modelSelector') {
      setMessages(prev => ({ ...prev, [name]: '' }));
    }
  }, []);

  // Hide all modals
  const hideAllModals = useCallback(() => {
    setModals({
      modelSettings: false,
      deviceSettings: false,
      languageSettings: false,
      modelSelector: false,
      errorAlert: false,
      chunkDropWarning: false,
    });
    setMessages({
      errorAlert: '',
      chunkDropWarning: '',
      modelSelector: '',
    });
  }, []);

  // Set up chunk loss listener. The backend emits `transcript-chunk-loss-detected`;
  // `chunk-drop-warning` was never emitted, so this listener could not fire.
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;

    const setupChunkLossListener = async () => {
      try {
        console.log('Setting up transcript-chunk-loss-detected listener...');
        unlistenFn = await listen<ChunkLossPayload | null>('transcript-chunk-loss-detected', (event) => {
          const payload = event.payload ?? {};
          console.warn('Transcript chunk loss detected:', payload);
          const title = tr('chunkLoss.title');
          const body = tr('chunkLoss.body');
          const detail = typeof payload.message === 'string' ? payload.message.trim() : '';
          // Show the localized explanation; append the backend's own message only
          // when it carries context the generic copy does not repeat.
          const text = detail && !body.includes(detail) && !title.includes(detail)
            ? `${title}. ${body} ${detail}`
            : `${title}. ${body}`;
          showModal('chunkDropWarning', text);
        });
        console.log('Transcript chunk loss listener setup complete');
      } catch (error) {
        console.error('Failed to setup transcript chunk loss listener:', error);
      }
    };

    setupChunkLossListener();

    return () => {
      console.log('Cleaning up transcript chunk loss listener...');
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [showModal]);

  // Set up transcription error listener for model loading failures
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;

    const setupTranscriptionErrorListener = async () => {
      try {
        console.log('Setting up transcription-error listener...');
        unlistenFn = await listen<TranscriptionErrorPayload>('transcription-error', (event) => {
          console.log('Transcription error received:', event.payload);
          const { userMessage, actionable } = event.payload;

          if (actionable) {
            // This is a model-related error that requires user action
            showModal('modelSelector', userMessage);
          } else {
            // Show toast instead of modal for non-actionable errors (consistent with sidebar)
            toast.error('', {
              description: userMessage,
              duration: 5000,
            });
          }
        });
        console.log('Transcription error listener setup complete');
      } catch (error) {
        console.error('Failed to setup transcription error listener:', error);
      }
    };

    setupTranscriptionErrorListener();

    return () => {
      console.log('Cleaning up transcription error listener...');
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [showModal]);

  // Listen for model download completion to auto-close modal
  useEffect(() => {
    const setupDownloadListeners = async () => {
      const unlisteners: (() => void)[] = [];

      // Listen for Whisper model download complete
      const unlistenWhisper = await listen<{ modelName: string }>('model-download-complete', (event) => {
        const { modelName } = event.payload;
        console.log('[useModalState] Whisper model download complete:', modelName);

        // Auto-close modal if the downloaded model matches the selected one
        if (transcriptModelConfig?.provider === 'localWhisper' && transcriptModelConfig?.model === modelName) {
          toast.success('Model ready! Closing window...', { duration: 1500 });
          setTimeout(() => hideModal('modelSelector'), 1500);
        }
      });
      unlisteners.push(unlistenWhisper);

      return () => {
        unlisteners.forEach(unsub => unsub());
      };
    };

    setupDownloadListeners();
  }, [transcriptModelConfig, hideModal]);

  return {
    modals,
    messages,
    showModal,
    hideModal,
    hideAllModals,
  };
}
