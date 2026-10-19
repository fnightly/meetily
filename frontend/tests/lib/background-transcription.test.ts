import { describe, expect, test } from 'bun:test';
import {
  describeRun,
  folderTitle,
  transcribeArgs,
} from '../../src/lib/background-transcription';

/**
 * Background transcription announces each finished run once. These pin which
 * engine a queued recording uses and what the user is told when a run drains,
 * without Tauri or a rendered toast.
 */

describe('folderTitle', () => {
  test('uses the last path component on either separator', () => {
    expect(folderTitle('/Users/a/Meetings/Standup 2026-10-19')).toBe('Standup 2026-10-19');
    expect(folderTitle('C:\\Users\\a\\Meetings\\Review\\')).toBe('Review');
  });
});

describe('transcribeArgs', () => {
  const settings = { provider: 'localWhisper', model: 'large-v3', language: 'zh' };

  test('uses the Settings engine and language when nothing was picked', () => {
    expect(transcribeArgs(settings, undefined, undefined)).toEqual({
      provider: null,
      model: 'large-v3',
      language: 'zh',
    });
  });

  test('a picked model and language override Settings', () => {
    expect(transcribeArgs(settings, { provider: 'groq', name: 'whisper-large-v3' }, 'en')).toEqual({
      provider: 'groq',
      model: 'whisper-large-v3',
      language: 'en',
    });
  });

  test('an explicit null language means automatic detection', () => {
    expect(transcribeArgs(settings, undefined, null).language).toBeNull();
  });

  test('Parakeet never receives a language', () => {
    expect(transcribeArgs(settings, { provider: 'parakeet', name: 'v3' }, 'en').language).toBeNull();
  });
});

describe('describeRun', () => {
  test('says nothing for a run that was only cancelled', () => {
    expect(describeRun('transcribe', { done: [], failed: [] })).toBeNull();
  });

  test('one finished recording links to its meeting', () => {
    const result = describeRun('transcribe', {
      done: [{ title: 'Standup', meetingId: 'm-1', segmentsCount: 12 }],
      failed: [],
    });
    expect(result?.kind).toBe('success');
    expect(result?.title).toContain('Standup');
    expect(result?.description).toContain('12');
    expect(result?.href).toBe('/meeting-details?id=m-1');
  });

  test('one failure carries the backend message', () => {
    const result = describeRun('import', { done: [], failed: [{ title: 'call.mp3', message: 'decode failed' }] });
    expect(result?.kind).toBe('error');
    expect(result?.title).toContain('call.mp3');
    expect(result?.description).toBe('decode failed');
  });

  test('several results summarise and link to the library', () => {
    const result = describeRun('transcribe', {
      done: [{ title: 'a', meetingId: 'm-1' }, { title: 'b', meetingId: 'm-2' }],
      failed: [{ title: 'c', message: 'x' }],
    });
    expect(result?.kind).toBe('partial');
    expect(result?.title).toContain('3');
    expect(result?.description).toContain('2');
    expect(result?.href).toBe('/recordings');
  });
});
