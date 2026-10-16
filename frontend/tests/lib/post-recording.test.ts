import { describe, test, expect } from 'bun:test';
import {
  buildPostRecordingPrompt,
  actionNeedsTranscription,
  actionNeedsSummary,
  shouldSaveMeetingWithoutTranscripts,
  type PostRecordingAction,
} from '../../src/lib/post-recording';

describe('buildPostRecordingPrompt', () => {
  test('offers transcription and summarising when audio has no text yet', () => {
    const prompt = buildPostRecordingPrompt({
      deferred: true,
      transcriptCount: 0,
      hasAudio: true,
    });

    expect(prompt.show).toBe(true);
    const actions = prompt.choices.map((c) => c.action);
    expect(actions).toContain('transcribe');
    expect(actions).toContain('transcribe-and-summarize');
    expect(actions).toContain('keep');
  });

  test('explains that transcription was deferred', () => {
    const prompt = buildPostRecordingPrompt({
      deferred: true,
      transcriptCount: 0,
      hasAudio: true,
    });
    expect(prompt.message.toLowerCase()).toContain('deferred');
  });

  test('offers only summarising when text already exists', () => {
    const prompt = buildPostRecordingPrompt({
      deferred: false,
      transcriptCount: 42,
      hasAudio: true,
    });

    const actions = prompt.choices.map((c) => c.action);
    // Re-transcribing would discard work the user already has.
    expect(actions).not.toContain('transcribe');
    expect(actions).toEqual(['summarize', 'keep']);
  });

  test('always includes a way to decline', () => {
    for (const input of [
      { deferred: true, transcriptCount: 0, hasAudio: true },
      { deferred: false, transcriptCount: 5, hasAudio: true },
    ]) {
      expect(buildPostRecordingPrompt(input).choices.some((c) => c.action === 'keep')).toBe(true);
    }
  });

  test('shows nothing when the recording produced neither audio nor text', () => {
    // A prompt about an empty recording has nothing to offer and would only
    // confuse the user.
    const prompt = buildPostRecordingPrompt({
      deferred: true,
      transcriptCount: 0,
      hasAudio: false,
    });

    expect(prompt.show).toBe(false);
    expect(prompt.choices).toEqual([]);
  });

  test('still offers summarising when only text survived', () => {
    // Plausible if audio writing failed but live captions were captured.
    const prompt = buildPostRecordingPrompt({
      deferred: false,
      transcriptCount: 3,
      hasAudio: false,
    });

    expect(prompt.show).toBe(true);
    expect(prompt.choices.map((c) => c.action)).toContain('summarize');
  });

  test('every choice carries a label and a description', () => {
    const prompt = buildPostRecordingPrompt({
      deferred: true,
      transcriptCount: 0,
      hasAudio: true,
    });

    for (const choice of prompt.choices) {
      expect(choice.label.length).toBeGreaterThan(0);
      expect(choice.description.length).toBeGreaterThan(0);
    }
  });
});

describe('actionNeedsTranscription', () => {
  test('is true only for the actions that run speech-to-text', () => {
    const expected: Record<PostRecordingAction, boolean> = {
      transcribe: true,
      'transcribe-and-summarize': true,
      summarize: false,
      keep: false,
    };

    for (const [action, needs] of Object.entries(expected)) {
      expect(actionNeedsTranscription(action as PostRecordingAction)).toBe(needs);
    }
  });
});

describe('actionNeedsSummary', () => {
  test('is true only for the actions that write a summary', () => {
    const expected: Record<PostRecordingAction, boolean> = {
      transcribe: false,
      'transcribe-and-summarize': true,
      summarize: true,
      keep: false,
    };

    for (const [action, needs] of Object.entries(expected)) {
      expect(actionNeedsSummary(action as PostRecordingAction)).toBe(needs);
    }
  });

  test('transcribe-and-summarize requires both steps in order', () => {
    // The summary is written from the transcript, so it must not run first.
    expect(actionNeedsTranscription('transcribe-and-summarize')).toBe(true);
    expect(actionNeedsSummary('transcribe-and-summarize')).toBe(true);
  });
});

describe('shouldSaveMeetingWithoutTranscripts', () => {
  test('saves a deferred recording so it can be acted on later', () => {
    // This is the case that previously produced orphaned audio on disk.
    expect(
      shouldSaveMeetingWithoutTranscripts({ deferred: true, transcriptCount: 0, hasAudio: true }),
    ).toBe(true);
  });

  test('does not double-save when transcripts already exist', () => {
    expect(
      shouldSaveMeetingWithoutTranscripts({ deferred: false, transcriptCount: 10, hasAudio: true }),
    ).toBe(false);
  });

  test('does not save when there is no audio to keep', () => {
    expect(
      shouldSaveMeetingWithoutTranscripts({ deferred: true, transcriptCount: 0, hasAudio: false }),
    ).toBe(false);
  });
});
