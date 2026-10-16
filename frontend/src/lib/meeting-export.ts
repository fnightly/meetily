import type { MeetingMetadata, MeetingSummary, Section } from '@/types';

/**
 * Markdown export for a meeting: summary, full transcript, or both.
 *
 * This is deliberately independent from the interactive summary editor: the
 * editor holds a live block structure, whereas export must work from persisted
 * data (which may be a markdown string, BlockNote JSON, or the legacy
 * section/block shape depending on when the summary was generated).
 */

/**
 * The minimal segment shape needed for export.
 *
 * `api_get_meeting` returns complete transcripts (unlike the paginated list the
 * UI renders), and its `MeetingTranscript` omits `sequence_id` / `is_partial`,
 * so export works off this narrower contract that both types satisfy.
 */
export interface ExportSegment {
  text: string;
  timestamp?: string;
  sequence_id?: number;
  audio_start_time?: number;
  audio_end_time?: number;
}

export interface MeetingExportInput {
  meeting: Pick<MeetingMetadata, 'id' | 'title' | 'created_at'> &
    Partial<Pick<MeetingMetadata, 'updated_at' | 'folder_path'>>;
  summary?: MeetingSummary | null;
  transcripts?: ExportSegment[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** `mm:ss` for durations under an hour, `h:mm:ss` beyond. */
export function formatOffset(seconds: number | null | undefined): string {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) {
    return '';
  }
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = String(m).padStart(2, '0');
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

/** A per-segment label: recording-relative offset when available. */
function segmentTimestamp(segment: ExportSegment): string {
  const offset = formatOffset(segment.audio_start_time);
  if (offset) return offset;
  return segment.timestamp ?? '';
}

/**
 * Flatten a legacy `{ sectionKey: { title, blocks[] } }` summary.
 *
 * Returns `null` when the object does not have that shape, so callers can fall
 * through to another representation instead of throwing.
 */
function legacySectionsToMarkdown(summary: MeetingSummary): string | null {
  if (!isRecord(summary)) return null;

  const skip = new Set(['MeetingName', '_section_order', 'markdown', 'summary_json']);

  // Prefer the recorded order when the backend provided one.
  const order = Array.isArray((summary as Record<string, unknown>)._section_order)
    ? ((summary as Record<string, unknown>)._section_order as unknown[]).filter(
        (k): k is string => typeof k === 'string',
      )
    : [];

  const keys = order.length > 0 ? order : Object.keys(summary);
  const parts: string[] = [];

  for (const key of keys) {
    if (skip.has(key)) continue;
    const value = (summary as Record<string, unknown>)[key];
    if (!isRecord(value)) continue;

    const section = value as unknown as Section;
    const blocks = Array.isArray(section.blocks) ? section.blocks : [];
    if (blocks.length === 0) continue;

    parts.push(`## ${section.title || key}\n`);

    for (const block of blocks) {
      const content = typeof block?.content === 'string' ? block.content.trim() : '';
      if (!content) continue;

      switch (block?.type) {
        case 'heading1':
          parts.push(`### ${content}\n`);
          break;
        case 'heading2':
          parts.push(`#### ${content}\n`);
          break;
        case 'bullet':
          parts.push(`- ${content}`);
          break;
        case 'text':
        default:
          parts.push(`${content}\n`);
      }
    }

    // Separate bullet runs from the next section.
    if (blocks.some((b) => b?.type === 'bullet')) {
      parts.push('');
    }
  }

  return parts.length > 0 ? parts.join('\n') : null;
}

/** Flatten BlockNote JSON into markdown, preserving nesting and common types. */
function blockNoteToMarkdown(blocks: unknown, depth = 0): string[] {
  if (!Array.isArray(blocks)) return [];
  const lines: string[] = [];
  const indent = '  '.repeat(depth);

  for (const raw of blocks) {
    if (!isRecord(raw)) continue;

    const type = typeof raw.type === 'string' ? raw.type : 'paragraph';
    const text = blockNoteInlineToMarkdown(raw.content);

    switch (type) {
      case 'heading': {
        const level = isRecord(raw.props) && typeof raw.props.level === 'number' ? raw.props.level : 2;
        const clamped = Math.min(Math.max(level, 1), 6);
        if (text) lines.push(`${'#'.repeat(clamped)} ${text}\n`);
        break;
      }
      case 'bulletListItem':
        if (text) lines.push(`${indent}- ${text}`);
        break;
      case 'numberedListItem':
        if (text) lines.push(`${indent}1. ${text}`);
        break;
      case 'checkListItem': {
        const checked = isRecord(raw.props) && raw.props.checked === true;
        if (text) lines.push(`${indent}- [${checked ? 'x' : ' '}] ${text}`);
        break;
      }
      case 'codeBlock':
        if (text) lines.push('```\n' + text + '\n```\n');
        break;
      case 'quote':
        if (text) lines.push(`> ${text}\n`);
        break;
      case 'image': {
        const props = isRecord(raw.props) ? raw.props : {};
        const url = typeof props.url === 'string' ? props.url : '';
        const caption = typeof props.caption === 'string' ? props.caption : '';
        if (url) lines.push(`![${caption}](${url})\n`);
        break;
      }
      default:
        if (text) lines.push(`${indent}${text}\n`);
    }

    const children = blockNoteToMarkdown(raw.children, depth + 1);
    if (children.length > 0) lines.push(...children);
  }

  return lines;
}

/** Flatten BlockNote inline content (which may carry styling links). */
function blockNoteInlineToMarkdown(content: unknown): string {
  if (typeof content === 'string') return content.trim();
  if (!Array.isArray(content)) return '';

  return content
    .map((item) => {
      if (typeof item === 'string') return item;
      if (!isRecord(item)) return '';
      if (typeof item.text === 'string') {
        // Preserve links when the inline node carries one.
        const link = isRecord(item.link) && typeof item.link.url === 'string' ? item.link.url : null;
        return link ? `[${item.text}](${link})` : item.text;
      }
      return '';
    })
    .join('')
    .trim();
}

/** Render whatever summary representation we were handed. */
export function summaryToMarkdown(summary: MeetingSummary | null | undefined): string {
  if (!summary) return '';

  if (isRecord(summary)) {
    if (typeof summary.markdown === 'string' && summary.markdown.trim()) {
      return summary.markdown.trim();
    }
    if (Array.isArray(summary.summary_json)) {
      const rendered = blockNoteToMarkdown(summary.summary_json).join('\n').trim();
      if (rendered) return rendered;
    }
  }

  return legacySectionsToMarkdown(summary)?.trim() ?? '';
}

/** Group transcript segments into a readable markdown transcript. */
export function transcriptToMarkdown(transcripts: ExportSegment[] | undefined): string {
  if (!transcripts || transcripts.length === 0) return '';

  const ordered = [...transcripts].sort((a, b) => {
    const aStart = a.audio_start_time ?? Number.POSITIVE_INFINITY;
    const bStart = b.audio_start_time ?? Number.POSITIVE_INFINITY;
    if (aStart !== bStart) return aStart - bStart;
    return (a.sequence_id ?? 0) - (b.sequence_id ?? 0);
  });

  return ordered
    .map((segment) => {
      const text = (segment.text ?? '').trim();
      if (!text) return '';
      const stamp = segmentTimestamp(segment);
      return stamp ? `**${stamp}** ${text}` : text;
    })
    .filter(Boolean)
    .join('\n\n');
}

export type ExportScope = 'summary' | 'transcript' | 'both';

/**
 * Build the complete markdown document for a meeting.
 *
 * `scope` decides which parts are included; the other parts are omitted
 * entirely rather than left as empty headings.
 */
export function buildMeetingMarkdown(
  { meeting, summary, transcripts }: MeetingExportInput,
  scope: ExportScope = 'both',
): string {
  const title = meeting.title?.trim() || 'Untitled Meeting';
  const parts: string[] = [`# ${title}\n`];

  const meta: string[] = [];
  if (meeting.created_at) {
    const created = new Date(meeting.created_at);
    meta.push(
      `- **Date:** ${
        Number.isNaN(created.getTime()) ? meeting.created_at : created.toLocaleString()
      }`,
    );
  }
  meta.push(`- **Meeting ID:** \`${meeting.id}\``);
  if (transcripts && transcripts.length > 0) {
    meta.push(`- **Segments:** ${transcripts.length}`);
  }
  parts.push(meta.join('\n') + '\n');

  if (scope === 'summary' || scope === 'both') {
    const summaryMd = summaryToMarkdown(summary);
    parts.push('## Summary\n');
    parts.push(summaryMd ? summaryMd + '\n' : '_No summary has been generated for this meeting._\n');
  }

  if (scope === 'transcript' || scope === 'both') {
    const transcriptMd = transcriptToMarkdown(transcripts);
    parts.push('## Transcript\n');
    parts.push(
      transcriptMd ? transcriptMd + '\n' : '_No transcript is available for this meeting._\n',
    );
  }

  return parts.join('\n');
}

/** Filesystem-safe base name for the downloaded file. */
export function exportFileName(meeting: Pick<MeetingMetadata, 'id' | 'title'>, scope: ExportScope): string {
  const raw = meeting.title?.trim() || meeting.id || 'meeting';
  const safe = raw
    .replace(/[\\/:*?"<>|]/g, '-')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 80);
  const suffix = scope === 'both' ? '' : `-${scope}`;
  return `${safe || 'meeting'}${suffix}.md`;
}

/** Trigger a browser download of the generated markdown. */
export function downloadMarkdown(markdown: string, filename: string): void {
  const blob = new Blob([markdown], { type: 'text/markdown;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  document.body.removeChild(anchor);
  URL.revokeObjectURL(url);
}
