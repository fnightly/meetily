import { describe, expect, mock, test } from 'bun:test';

mock.module('@tauri-apps/api/core', () => ({ invoke: async () => ({ speaker_1: '张总' }) }));
const { displaySpeaker, speakerLabelsIn, fetchSpeakerNames } = await import('../../src/lib/speaker-names');

const t = (key: string, params?: Record<string, unknown>) => (key === 'speakers.label' ? `说话人 ${params?.n}` : key);

describe('speaker names', () => {
  test('uses the given name, else the numbered label, else the raw label', () => {
    expect(displaySpeaker('speaker_1', t, { speaker_1: '张总' })).toBe('张总');
    expect(displaySpeaker('speaker_2', t, { speaker_1: '张总' })).toBe('说话人 2');
    expect(displaySpeaker('speaker_2', t, { speaker_2: '   ' })).toBe('说话人 2');
    expect(displaySpeaker('host', t, null)).toBe('host');
  });

  test('lists labels in first-appearance order', () => {
    expect(speakerLabelsIn([{ speaker: 'speaker_2' }, { speaker: null }, { speaker: 'speaker_1' }, { speaker: 'speaker_2' }]))
      .toEqual(['speaker_2', 'speaker_1']);
  });

  test('fetches names through the backend command', async () => {
    expect(await fetchSpeakerNames('m1')).toEqual({ speaker_1: '张总' });
  });
});
