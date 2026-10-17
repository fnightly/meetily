/**
 * Which engine a library transcription should use.
 *
 * The retranscription command falls back to Whisper when it is not told
 * otherwise, so passing nothing would transcribe a Parakeet or cloud setup with
 * a local Whisper model it may not even have downloaded. These helpers turn the
 * active speech settings into the command arguments the batch path understands.
 */

/** The shape returned by `api_get_active_speech_settings`. */
export interface ActiveSpeechSettings {
  provider: string;
  model: string;
  /** Raw preference: `auto`, `auto-translate`, or an ISO code. */
  language: string | null;
}

export interface BatchTranscriptionTarget {
  /** Provider id, or null to let the command pick its configured Whisper engine. */
  provider: string | null;
  /** Model id, or null to use the provider's configured model. */
  model: string | null;
  /** A concrete language code, or null for automatic detection. */
  language: string | null;
}

/**
 * The command arguments for the active speech settings.
 *
 * `localWhisper` is the batch path's own default, so it is sent as null; every
 * other id is passed through — `parakeet` selects the Parakeet engine, and cloud
 * ids are resolved by the batch path. Raw values starting with `auto` are
 * preference modes rather than language codes and must not be sent as one.
 */
export function batchTranscriptionTarget(
  settings: ActiveSpeechSettings | null | undefined,
): BatchTranscriptionTarget {
  if (!settings) {
    return { provider: null, model: null, language: null };
  }

  const provider = settings.provider && settings.provider !== 'localWhisper'
    ? settings.provider
    : null;
  const model = settings.model && settings.model.trim() ? settings.model.trim() : null;
  const language = settings.language && !settings.language.startsWith('auto')
    ? settings.language
    : null;

  return { provider, model, language };
}
