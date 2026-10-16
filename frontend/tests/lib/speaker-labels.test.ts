import { describe, expect, test } from 'bun:test';
import {
  formatSpeakerLabel,
  isStoredSpeakerLabel,
  speakerNumberOf,
} from '../../src/lib/speaker-labels';
import { translate } from '../../src/lib/i18n';

/**
 * The stored `speaker_N` form is shared by the transcript list, the exported document and
 * the clipboard, so the parsing and the wording live in one place.
 */

const t = (key: string, params?: Record<string, string | number>) => translate('zh-CN', key, params);

describe('speaker label parsing', () => {
  test('extracts the number from a stored label', () => {
    expect(speakerNumberOf('speaker_1')).toBe('1');
    expect(speakerNumberOf('speaker_12')).toBe('12');
  });

  test('anything that is not the stored shape yields null', () => {
    // Null rather than a default: guessing a number would attribute speech to the wrong
    // person, so callers show the label verbatim instead.
    for (const bad of ['Speaker_1', 'speaker', 'speaker_', 'speaker_1a', '1', '', 'speaker_ 1']) {
      expect(speakerNumberOf(bad)).toBeNull();
    }
  });

  test('isStoredSpeakerLabel recognises only the stored shape', () => {
    expect(isStoredSpeakerLabel('speaker_1')).toBe(true);
    expect(isStoredSpeakerLabel('Unknown')).toBe(false);
    expect(isStoredSpeakerLabel(null)).toBe(false);
    expect(isStoredSpeakerLabel(undefined)).toBe(false);
  });
});

describe('speaker label formatting', () => {
  test('a stored label becomes words in the interface language', () => {
    expect(formatSpeakerLabel('speaker_1', t)).toBe('说话人 1');
    expect(formatSpeakerLabel('speaker_9', t)).toBe('说话人 9');
  });

  test('an unparseable label is returned unchanged', () => {
    // It is still information; replacing it with a number would be a fabrication.
    expect(formatSpeakerLabel('Unknown', t)).toBe('Unknown');
  });

  test('the same input gives the same output on every call', () => {
    // The export and the list must not disagree about a label.
    expect(formatSpeakerLabel('speaker_3', t)).toBe(formatSpeakerLabel('speaker_3', t));
  });

  test('the English catalogue words it differently', () => {
    const en = (key: string, params?: Record<string, string | number>) => translate('en', key, params);
    expect(formatSpeakerLabel('speaker_2', en)).toBe('Speaker 2');
  });
});
