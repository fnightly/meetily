/**
 * What to offer once a recording has stopped.
 *
 * The app previously decided this implicitly: if live transcription had produced
 * text it saved a meeting, and in deferred-transcription mode it saved nothing at
 * all — the audio landed on disk with no database record, so it could not be
 * opened or retranscribed from the UI. There was also no way to ask for a summary
 * at the one moment the user knows whether one is wanted.
 *
 * This module decides which choices to present. It is pure so the rules can be
 * tested without a recording.
 */

/**
 * Whether the recording needs saving as a meeting even though it has no text.
 *
 * A recording was previously only saved when live transcription had produced
 * segments, so a deferred-transcription session left the audio on disk with no
 * meeting row — unreachable from the UI and impossible to retranscribe. Saving it
 * is what makes the later steps possible.
 */
export function shouldSaveMeetingWithoutTranscripts(input: PostRecordingInput): boolean {
  return input.hasAudio && input.transcriptCount === 0;
}

export interface PostRecordingInput {
  /** Whether realtime transcription was skipped for this recording. */
  deferred: boolean;
  /** Segments produced by live transcription (0 when deferred, or on silence). */
  transcriptCount: number;
  /** Whether audio was actually written to disk. */
  hasAudio: boolean;
}

export type PostRecordingAction = 'transcribe' | 'transcribe-and-summarize' | 'summarize' | 'keep';

export interface PostRecordingChoice {
  action: PostRecordingAction;
  label: string;
  description: string;
}

export interface PostRecordingPrompt {
  /** Whether to show the prompt at all. */
  show: boolean;
  title: string;
  message: string;
  choices: PostRecordingChoice[];
}

const KEEP: PostRecordingChoice = {
  action: 'keep',
  label: 'Keep audio only',
  description: 'No transcription or summary. You can do either later from the meeting.',
};

const TRANSCRIBE: PostRecordingChoice = {
  action: 'transcribe',
  label: 'Transcribe',
  description: 'Turn the recording into text. Runs on the configured speech model.',
};

const TRANSCRIBE_AND_SUMMARIZE: PostRecordingChoice = {
  action: 'transcribe-and-summarize',
  label: 'Transcribe and summarise',
  description: 'Transcribe first, then write a summary from the text.',
};

const SUMMARIZE: PostRecordingChoice = {
  action: 'summarize',
  label: 'Summarise',
  description: 'Write a summary from the text already captured during the recording.',
};

/**
 * Build the prompt for a finished recording.
 *
 * Returns `show: false` when the recording produced nothing worth acting on — a
 * recording with neither audio nor text has nothing downstream to offer, and a
 * prompt about it would only be confusing.
 */
export function buildPostRecordingPrompt(input: PostRecordingInput): PostRecordingPrompt {
  const hasTranscript = input.transcriptCount > 0;

  if (!input.hasAudio && !hasTranscript) {
    return {
      show: false,
      title: '',
      message: '',
      choices: [],
    };
  }

  // Audio with no text: both steps are still available, so offer the full set.
  if (input.hasAudio && !hasTranscript) {
    return {
      show: true,
      title: 'Recording finished',
      message: input.deferred
        ? 'Transcription was deferred for this recording. Choose what to do with it now.'
        : 'No speech was captured for transcription. Choose what to do with the audio.',
      choices: [TRANSCRIBE_AND_SUMMARIZE, TRANSCRIBE, KEEP],
    };
  }

  // Text already exists, so transcribing again would be redundant. Summarising
  // is the only meaningful next step besides stopping.
  return {
    show: true,
    title: 'Recording finished',
    message: 'Transcription is already complete. Would you like a summary?',
    choices: [SUMMARIZE, KEEP],
  };
}

/** Whether an action requires running speech-to-text over the saved audio. */
export function actionNeedsTranscription(action: PostRecordingAction): boolean {
  return action === 'transcribe' || action === 'transcribe-and-summarize';
}

/** Whether an action requires generating a summary. */
export function actionNeedsSummary(action: PostRecordingAction): boolean {
  return action === 'summarize' || action === 'transcribe-and-summarize';
}
