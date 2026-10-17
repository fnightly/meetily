import { describe, expect, test } from 'bun:test';
import { formatSyncTags, parseSyncTags } from '../../src/lib/sync';

describe('parseSyncTags', () => {
  test('splits on commas, including the Chinese comma', () => {
    expect(parseSyncTags('meeting, client，Q3')).toEqual(['meeting', 'client', 'Q3']);
  });

  test('accepts newlines, semicolons and stray whitespace', () => {
    // Tags are often pasted from a note, where they are one per line.
    expect(parseSyncTags(' meeting \n client ; Q3 ')).toEqual(['meeting', 'client', 'Q3']);
  });

  test('strips a leading hash and blank entries', () => {
    expect(parseSyncTags('#meeting, , ##client')).toEqual(['meeting', 'client']);
  });

  test('deduplicates case-insensitively because Obsidian folds tag case', () => {
    expect(parseSyncTags('Meeting, meeting, MEETING')).toEqual(['Meeting']);
  });

  test('caps the count at 20 and each tag at 64 characters', () => {
    const many = Array.from({ length: 30 }, (_, index) => `tag${index}`).join(', ');
    expect(parseSyncTags(many)).toHaveLength(20);

    const [long] = parseSyncTags('x'.repeat(100));
    expect(Array.from(long)).toHaveLength(64);
  });

  test('returns an empty list for empty input', () => {
    expect(parseSyncTags('')).toEqual([]);
    expect(parseSyncTags('   ,  ')).toEqual([]);
  });
});

describe('formatSyncTags', () => {
  test('renders stored tags back into the editable field', () => {
    expect(formatSyncTags(['meeting', 'Q3 规划'])).toBe('meeting, Q3 规划');
  });

  test('tolerates missing tags', () => {
    expect(formatSyncTags(undefined)).toBe('');
    expect(formatSyncTags([])).toBe('');
  });
});
