import { describe, expect, test } from 'bun:test';
import {
  buildMeetingMarkdown,
  exportFileName,
  formatOffset,
  summaryToMarkdown,
  transcriptToMarkdown,
} from '../../src/lib/meeting-export';

const meeting = {
  id: 'meeting-1',
  title: 'Weekly Sync',
  created_at: '2026-02-03T10:00:00.000Z',
};

describe('formatOffset', () => {
  test('formats sub-hour offsets as mm:ss', () => {
    expect(formatOffset(0)).toBe('00:00');
    expect(formatOffset(65)).toBe('01:05');
    expect(formatOffset(3599)).toBe('59:59');
  });

  test('formats hour-plus offsets as h:mm:ss', () => {
    expect(formatOffset(3600)).toBe('1:00:00');
    expect(formatOffset(3725)).toBe('1:02:05');
  });

  test('returns empty string for missing or invalid values', () => {
    expect(formatOffset(undefined)).toBe('');
    expect(formatOffset(null)).toBe('');
    expect(formatOffset(Number.NaN)).toBe('');
    expect(formatOffset(-5)).toBe('');
  });
});

describe('summaryToMarkdown', () => {
  test('passes through a markdown summary unchanged', () => {
    expect(summaryToMarkdown({ markdown: '## Heading\n\nbody' })).toBe('## Heading\n\nbody');
  });

  test('renders BlockNote JSON including nested children and task state', () => {
    const summary = {
      summary_json: [
        { id: 'b1', type: 'heading', props: { level: 2 }, content: 'Decisions' },
        { id: 'b2', type: 'bulletListItem', content: 'Ship on Friday' },
        { id: 'b3', type: 'checkListItem', props: { checked: true }, content: 'Notify support' },
        {
          id: 'b4',
          type: 'bulletListItem',
          content: 'Parent',
          children: [{ id: 'b5', type: 'bulletListItem', content: 'Child' }],
        },
      ],
    };
    const md = summaryToMarkdown(summary);
    expect(md).toContain('## Decisions');
    expect(md).toContain('- Ship on Friday');
    expect(md).toContain('- [x] Notify support');
    // Nested items must be indented, not flattened to the top level.
    expect(md).toContain('  - Child');
  });

  test('preserves inline links in BlockNote content', () => {
    const summary = {
      summary_json: [
        {
          id: 'b1',
          type: 'paragraph',
          content: [{ type: 'link', text: 'spec', link: { url: 'https://example.com/spec' } }],
        },
      ],
    };
    expect(summaryToMarkdown(summary)).toContain('[spec](https://example.com/spec)');
  });

  test('renders the legacy section/block shape and respects _section_order', () => {
    const summary = {
      MeetingName: 'Weekly Sync',
      _section_order: ['second', 'first'],
      first: { title: 'First Section', blocks: [{ id: 'a', type: 'text', content: 'alpha', color: '' }] },
      second: { title: 'Second Section', blocks: [{ id: 'b', type: 'bullet', content: 'beta', color: '' }] },
    };
    const md = summaryToMarkdown(summary);
    // Ordering follows _section_order, and metadata keys are not emitted.
    expect(md.indexOf('## Second Section')).toBeLessThan(md.indexOf('## First Section'));
    expect(md).not.toContain('MeetingName');
  });

  test('returns empty string for null/empty summaries instead of throwing', () => {
    expect(summaryToMarkdown(null)).toBe('');
    expect(summaryToMarkdown(undefined)).toBe('');
    expect(summaryToMarkdown({})).toBe('');
  });
});

describe('transcriptToMarkdown', () => {
  test('orders segments by audio offset and labels them', () => {
    const md = transcriptToMarkdown([
      { text: 'second', audio_start_time: 90 },
      { text: 'first', audio_start_time: 5 },
    ]);
    const lines = md.split('\n\n');
    expect(lines[0]).toBe('**00:05** first');
    expect(lines[1]).toBe('**01:30** second');
  });

  test('falls back to the wall-clock timestamp when no offset exists', () => {
    expect(transcriptToMarkdown([{ text: 'hello', timestamp: '14:30:05' }])).toBe(
      '**14:30:05** hello',
    );
  });

  test('uses sequence_id to break ties on equal offsets', () => {
    const md = transcriptToMarkdown([
      { text: 'later', audio_start_time: 10, sequence_id: 2 },
      { text: 'earlier', audio_start_time: 10, sequence_id: 1 },
    ]);
    expect(md.indexOf('earlier')).toBeLessThan(md.indexOf('later'));
  });

  test('drops blank segments and handles an empty list', () => {
    expect(transcriptToMarkdown([{ text: '   ' }])).toBe('');
    expect(transcriptToMarkdown([])).toBe('');
    expect(transcriptToMarkdown(undefined)).toBe('');
  });

  test('a speaker is written into the transcript', () => {
    // Without this the exported file loses who said what, even though the interface showed
    // it — the export is the artefact people keep and share.
    const md = transcriptToMarkdown([
      { text: 'hello', audio_start_time: 5, speaker: 'speaker_1' },
    ]);

    expect(md).toBe('**00:05 · speaker_1** hello');
  });

  test('a caller-supplied formatter turns the stored label into words', () => {
    const md = transcriptToMarkdown(
      [{ text: 'hello', audio_start_time: 5, speaker: 'speaker_2' }],
      (raw) => raw.replace('speaker_', '说话人 '),
    );

    expect(md).toBe('**00:05 · 说话人 2** hello');
  });

  test('the speaker is repeated on every line rather than grouped', () => {
    // A document gets quoted and reordered; a label that depended on the line above it
    // would not survive that, unlike in the list where grouping is a reading aid.
    const md = transcriptToMarkdown([
      { text: 'first', audio_start_time: 1, speaker: 'speaker_1' },
      { text: 'second', audio_start_time: 2, speaker: 'speaker_1' },
    ]);

    expect(md.split('speaker_1').length - 1).toBe(2);
  });

  test('a segment without a speaker keeps the original shape', () => {
    // Meetings that were never separated must export exactly as before.
    const md = transcriptToMarkdown([{ text: 'hello', audio_start_time: 5 }]);

    expect(md).toBe('**00:05** hello');
  });

  test('a speaker on a segment with no timestamp still appears', () => {
    const md = transcriptToMarkdown([{ text: 'hello', speaker: 'speaker_3' }]);

    expect(md).toBe('**speaker_3** hello');
  });

  test('an unparseable speaker is written verbatim rather than dropped', () => {
    const md = transcriptToMarkdown([{ text: 'hello', audio_start_time: 5, speaker: 'Unknown' }]);

    expect(md).toContain('Unknown');
  });
});

describe('buildMeetingMarkdown', () => {
  test('includes title, metadata, summary and transcript for "both"', () => {
    const md = buildMeetingMarkdown(
      {
        meeting,
        summary: { markdown: 'Key point' },
        transcripts: [{ text: 'spoken words', audio_start_time: 12 }],
      },
      'both',
    );
    expect(md).toContain('# Weekly Sync');
    expect(md).toContain('`meeting-1`');
    expect(md).toContain('## Summary');
    expect(md).toContain('Key point');
    expect(md).toContain('## Transcript');
    expect(md).toContain('**00:12** spoken words');
  });

  test('omits the transcript section entirely for scope "summary"', () => {
    const md = buildMeetingMarkdown(
      { meeting, summary: { markdown: 'only summary' }, transcripts: [{ text: 'nope' }] },
      'summary',
    );
    expect(md).toContain('## Summary');
    expect(md).not.toContain('## Transcript');
    expect(md).not.toContain('nope');
  });

  test('omits the summary section entirely for scope "transcript"', () => {
    const md = buildMeetingMarkdown(
      { meeting, summary: { markdown: 'hidden' }, transcripts: [{ text: 'kept' }] },
      'transcript',
    );
    expect(md).not.toContain('## Summary');
    expect(md).not.toContain('hidden');
    expect(md).toContain('## Transcript');
  });

  test('states absence explicitly rather than leaving empty headings', () => {
    const md = buildMeetingMarkdown({ meeting, summary: null, transcripts: [] }, 'both');
    expect(md).toContain('_No summary has been generated for this meeting._');
    expect(md).toContain('_No transcript is available for this meeting._');
  });

  test('falls back to a placeholder title when the meeting has none', () => {
    const md = buildMeetingMarkdown({ meeting: { ...meeting, title: '   ' } }, 'summary');
    expect(md).toContain('# Untitled Meeting');
  });
});

describe('exportFileName', () => {
  test('strips characters that are illegal in file names', () => {
    const name = exportFileName({ id: 'x', title: 'Q1/Q2: Review <final>' }, 'both');
    expect(name).toBe('Q1-Q2- Review -final-.md');
    expect(name).not.toMatch(/[\\/:*?"<>|]/);
  });

  test('adds a scope suffix when exporting a single part', () => {
    expect(exportFileName({ id: 'x', title: 'Sync' }, 'transcript')).toBe('Sync-transcript.md');
    expect(exportFileName({ id: 'x', title: 'Sync' }, 'both')).toBe('Sync.md');
  });

  test('falls back to the meeting id when the title is empty', () => {
    expect(exportFileName({ id: 'meeting-9', title: '  ' }, 'both')).toBe('meeting-9.md');
  });
});
