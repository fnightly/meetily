import type { TranslateParams } from '@/lib/i18n';

/**
 * Speaker labels, shared by everywhere a transcript is shown or written out.
 *
 * Labels are stored as `speaker_1`, `speaker_2`, … — a form chosen to be language-neutral
 * and sortable. Every surface that displays one has to turn it into words, and before this
 * existed the transcript list did so privately while the export and the clipboard dropped
 * the label entirely.
 */

/** Signature of the translation function the interface passes around. */
type Translate = (key: string, params?: TranslateParams) => string;

/**
 * The number in a stored `speaker_N` label, or `null` when the label is not that shape.
 *
 * Returns `null` rather than a default so a caller can show the label verbatim: a label the
 * interface cannot parse is still information, and substituting a number would attribute
 * speech to the wrong person.
 */
export function speakerNumberOf(label: string): string | null {
  const match = /^speaker_(\d+)$/.exec(label);
  return match ? match[1] : null;
}

/**
 * Turn a stored label into display text.
 *
 * An unparseable label is returned unchanged rather than dropped.
 */
export function formatSpeakerLabel(label: string, t: Translate): string {
  const number = speakerNumberOf(label);
  return number ? t('speakers.label', { n: number }) : label;
}

/** Whether a value is a label this module knows how to render. */
export function isStoredSpeakerLabel(label: string | null | undefined): boolean {
  return typeof label === 'string' && speakerNumberOf(label) !== null;
}
