/**
 * Multi-format meeting export: Markdown, plain text, Word (.docx), HTML
 * (print-ready, "Save as PDF" from any browser), SRT / WebVTT subtitles, JSON.
 *
 * Everything here is pure string building so it can be unit-tested without a
 * DOM or Tauri. Writing the files (and zipping the .docx parts) is done by the
 * Rust command `export_save_files`.
 */

import {
  buildMeetingMarkdown,
  DEFAULT_EXPORT_LABELS,
  formatOffset,
  meetingMetaPairs,
  summaryToMarkdown,
  type ExportLabels,
  type ExportScope,
  type ExportSegment,
  type MeetingExportInput,
} from './meeting-export';

export type ExportFormat = 'md' | 'txt' | 'docx' | 'html' | 'srt' | 'vtt' | 'json';

export const EXPORT_FORMATS: ExportFormat[] = ['docx', 'md', 'txt', 'html', 'srt', 'vtt', 'json'];

/** Subtitle formats carry only the transcript, so they ignore the scope. */
export function formatUsesScope(format: ExportFormat): boolean {
  return format !== 'srt' && format !== 'vtt';
}

/** Whether a format can be produced from what the meeting has. */
export function formatNeedsTranscript(format: ExportFormat): boolean {
  return format === 'srt' || format === 'vtt';
}

/** A file handed to the Rust writer. */
export type ExportFilePayload =
  | { name: string; kind: 'text'; text: string }
  | { name: string; kind: 'docx'; parts: Array<{ path: string; content: string }> };

// ---------------------------------------------------------------------------
// Markdown → blocks (the subset meeting summaries actually use)
// ---------------------------------------------------------------------------

export interface InlineRun {
  text: string;
  bold?: boolean;
  italic?: boolean;
  code?: boolean;
}

export type MdBlock =
  | { type: 'heading'; level: number; text: string }
  | { type: 'paragraph'; text: string }
  | { type: 'list'; ordered: boolean; items: Array<{ text: string; depth: number; checked?: boolean }> }
  | { type: 'table'; rows: string[][] }
  | { type: 'quote'; text: string }
  | { type: 'code'; text: string }
  | { type: 'hr' };

const LIST_RE = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const TABLE_SEP_RE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

function splitTableRow(line: string): string[] {
  let body = line.trim();
  if (body.startsWith('|')) body = body.slice(1);
  if (body.endsWith('|')) body = body.slice(0, -1);
  return body.split('|').map((c) => c.trim());
}

export function parseMarkdownBlocks(markdown: string): MdBlock[] {
  const lines = (markdown ?? '').replace(/\r\n?/g, '\n').split('\n');
  const blocks: MdBlock[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    const trimmed = line.trim();

    if (!trimmed) {
      i++;
      continue;
    }

    if (trimmed.startsWith('```')) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith('```')) {
        body.push(lines[i]);
        i++;
      }
      i++; // closing fence
      blocks.push({ type: 'code', text: body.join('\n') });
      continue;
    }

    const heading = /^(#{1,6})\s+(.*)$/.exec(trimmed);
    if (heading) {
      blocks.push({ type: 'heading', level: heading[1].length, text: heading[2].trim() });
      i++;
      continue;
    }

    if (/^(-{3,}|\*{3,}|_{3,})$/.test(trimmed)) {
      blocks.push({ type: 'hr' });
      i++;
      continue;
    }

    if (trimmed.includes('|') && i + 1 < lines.length && TABLE_SEP_RE.test(lines[i + 1])) {
      const rows: string[][] = [splitTableRow(trimmed)];
      i += 2;
      while (i < lines.length && lines[i].trim().includes('|') && lines[i].trim()) {
        rows.push(splitTableRow(lines[i]));
        i++;
      }
      blocks.push({ type: 'table', rows });
      continue;
    }

    if (trimmed.startsWith('>')) {
      const body: string[] = [];
      while (i < lines.length && lines[i].trim().startsWith('>')) {
        body.push(lines[i].trim().replace(/^>\s?/, ''));
        i++;
      }
      blocks.push({ type: 'quote', text: body.join(' ') });
      continue;
    }

    const listMatch = LIST_RE.exec(line);
    if (listMatch) {
      const ordered = /\d/.test(listMatch[2]);
      const items: Array<{ text: string; depth: number; checked?: boolean }> = [];
      while (i < lines.length) {
        const m = LIST_RE.exec(lines[i]);
        if (!m) break;
        let text = m[3].trim();
        let checked: boolean | undefined;
        const task = /^\[( |x|X)\]\s+(.*)$/.exec(text);
        if (task) {
          checked = task[1].toLowerCase() === 'x';
          text = task[2];
        }
        const depth = Math.floor(m[1].replace(/\t/g, '  ').length / 2);
        items.push({ text, depth, ...(checked === undefined ? {} : { checked }) });
        i++;
      }
      blocks.push({ type: 'list', ordered, items });
      continue;
    }

    // Paragraph: consecutive non-special lines.
    const body: string[] = [trimmed];
    i++;
    while (i < lines.length) {
      const next = lines[i].trim();
      if (
        !next ||
        next.startsWith('#') ||
        next.startsWith('>') ||
        next.startsWith('```') ||
        LIST_RE.test(lines[i]) ||
        (next.includes('|') && i + 1 < lines.length && TABLE_SEP_RE.test(lines[i + 1]))
      ) {
        break;
      }
      body.push(next);
      i++;
    }
    blocks.push({ type: 'paragraph', text: body.join(' ') });
  }

  return blocks;
}

/** Split `**bold**`, `*italic*` / `_italic_`, `` `code` `` and links into runs. */
export function parseInline(text: string): InlineRun[] {
  const runs: InlineRun[] = [];
  // Links keep their label; the URL is appended in parentheses for print.
  const source = (text ?? '').replace(/!\[([^\]]*)\]\(([^)]+)\)/g, '$1').replace(
    /\[([^\]]+)\]\(([^)]+)\)/g,
    (_m, label: string, url: string) => (label === url ? label : `${label} (${url})`),
  );
  const re = /(\*\*[^*]+\*\*|__[^_]+__|`[^`]+`|\*[^*\s][^*]*\*|_[^_\s][^_]*_)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(source))) {
    if (m.index > last) runs.push({ text: source.slice(last, m.index) });
    const token = m[0];
    if (token.startsWith('**') || token.startsWith('__')) {
      runs.push({ text: token.slice(2, -2), bold: true });
    } else if (token.startsWith('`')) {
      runs.push({ text: token.slice(1, -1), code: true });
    } else {
      runs.push({ text: token.slice(1, -1), italic: true });
    }
    last = m.index + token.length;
  }
  if (last < source.length) runs.push({ text: source.slice(last) });
  return runs.filter((r) => r.text.length > 0);
}

function inlinePlain(text: string): string {
  return parseInline(text)
    .map((r) => r.text)
    .join('');
}

// ---------------------------------------------------------------------------
// Plain text
// ---------------------------------------------------------------------------

export function markdownToPlainText(markdown: string): string {
  const out: string[] = [];
  for (const block of parseMarkdownBlocks(markdown)) {
    switch (block.type) {
      case 'heading': {
        const text = inlinePlain(block.text);
        out.push(block.level <= 2 ? `${text}\n${(block.level === 1 ? '=' : '-').repeat(Math.max(4, Math.min(40, text.length * 2)))}` : text);
        break;
      }
      case 'paragraph':
      case 'quote':
        out.push(inlinePlain(block.text));
        break;
      case 'code':
        out.push(block.text);
        break;
      case 'hr':
        out.push('—'.repeat(20));
        break;
      case 'list':
        out.push(
          block.items
            .map((item, idx) => {
              const marker =
                item.checked !== undefined ? (item.checked ? '[x]' : '[ ]') : block.ordered ? `${idx + 1}.` : '•';
              return `${'  '.repeat(item.depth)}${marker} ${inlinePlain(item.text)}`;
            })
            .join('\n'),
        );
        break;
      case 'table':
        out.push(block.rows.map((row) => row.map(inlinePlain).join('\t')).join('\n'));
        break;
    }
  }
  return out.join('\n\n') + '\n';
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function inlineHtml(text: string): string {
  return parseInline(text)
    .map((r) => {
      let html = escapeHtml(r.text);
      if (r.code) html = `<code>${html}</code>`;
      if (r.bold) html = `<strong>${html}</strong>`;
      if (r.italic) html = `<em>${html}</em>`;
      return html;
    })
    .join('');
}

export function markdownToHtml(markdown: string): string {
  const out: string[] = [];
  for (const block of parseMarkdownBlocks(markdown)) {
    switch (block.type) {
      case 'heading':
        out.push(`<h${block.level}>${inlineHtml(block.text)}</h${block.level}>`);
        break;
      case 'paragraph':
        out.push(`<p>${inlineHtml(block.text)}</p>`);
        break;
      case 'quote':
        out.push(`<blockquote>${inlineHtml(block.text)}</blockquote>`);
        break;
      case 'code':
        out.push(`<pre><code>${escapeHtml(block.text)}</code></pre>`);
        break;
      case 'hr':
        out.push('<hr />');
        break;
      case 'list': {
        const tag = block.ordered ? 'ol' : 'ul';
        const items = block.items
          .map((item) => {
            const box = item.checked !== undefined ? (item.checked ? '☑ ' : '☐ ') : '';
            const style = item.depth > 0 ? ` style="margin-left:${item.depth * 1.5}em"` : '';
            return `<li${style}>${box}${inlineHtml(item.text)}</li>`;
          })
          .join('');
        out.push(`<${tag}>${items}</${tag}>`);
        break;
      }
      case 'table': {
        const [head, ...rest] = block.rows;
        const th = head.map((c) => `<th>${inlineHtml(c)}</th>`).join('');
        const trs = rest.map((r) => `<tr>${r.map((c) => `<td>${inlineHtml(c)}</td>`).join('')}</tr>`).join('');
        out.push(`<table><thead><tr>${th}</tr></thead><tbody>${trs}</tbody></table>`);
        break;
      }
    }
  }
  return out.join('\n');
}

const HTML_STYLE = `
  body { font-family: -apple-system, "Segoe UI", "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
         max-width: 820px; margin: 40px auto; padding: 0 24px; color: #1f2937; line-height: 1.65; }
  h1 { font-size: 26px; margin-bottom: 4px; } h2 { font-size: 20px; border-bottom: 1px solid #e5e7eb; padding-bottom: 4px; margin-top: 32px; }
  h3 { font-size: 17px; } .meta { color: #6b7280; font-size: 13px; margin: 0 0 24px; padding: 0; list-style: none; }
  .meta li { display: inline-block; margin-right: 18px; } table { border-collapse: collapse; width: 100%; margin: 12px 0; }
  th, td { border: 1px solid #d1d5db; padding: 6px 10px; text-align: left; vertical-align: top; } th { background: #f3f4f6; }
  blockquote { border-left: 3px solid #d1d5db; margin: 0; padding-left: 12px; color: #4b5563; }
  code { background: #f3f4f6; padding: 1px 4px; border-radius: 3px; } pre { background: #f3f4f6; padding: 12px; overflow-x: auto; }
  .seg { margin: 0 0 10px; } .seg .who { color: #2563eb; font-weight: 600; font-size: 13px; margin-right: 6px; }
  .seg .ts { color: #9ca3af; font-size: 12px; font-variant-numeric: tabular-nums; margin-right: 6px; }
  .empty { color: #9ca3af; font-style: italic; }
  @media print { body { margin: 0 auto; } h2 { break-after: avoid; } .seg { break-inside: avoid; } }
`;

function orderedSegments(transcripts: ExportSegment[] | undefined): ExportSegment[] {
  return [...(transcripts ?? [])]
    .filter((s) => (s.text ?? '').trim())
    .sort((a, b) => {
      const aStart = a.audio_start_time ?? Number.POSITIVE_INFINITY;
      const bStart = b.audio_start_time ?? Number.POSITIVE_INFINITY;
      if (aStart !== bStart) return aStart - bStart;
      return (a.sequence_id ?? 0) - (b.sequence_id ?? 0);
    });
}

function speakerOf(input: MeetingExportInput, segment: ExportSegment): string {
  if (!segment.speaker) return '';
  return input.speakerLabel ? input.speakerLabel(segment.speaker) : segment.speaker;
}

function stampOf(segment: ExportSegment): string {
  return formatOffset(segment.audio_start_time) || segment.timestamp || '';
}

export function buildMeetingHtml(
  input: MeetingExportInput,
  scope: ExportScope = 'both',
  labels: ExportLabels = DEFAULT_EXPORT_LABELS,
  lang = 'zh-CN',
): string {
  const title = input.meeting.title?.trim() || labels.untitled;
  const meta = meetingMetaPairs(input, labels)
    .map(([k, v]) => `<li><strong>${escapeHtml(k)}:</strong> ${escapeHtml(v)}</li>`)
    .join('');
  const body: string[] = [`<h1>${escapeHtml(title)}</h1>`, `<ul class="meta">${meta}</ul>`];

  if (scope !== 'transcript') {
    const md = summaryToMarkdown(input.summary);
    body.push(`<h2>${escapeHtml(labels.summary)}</h2>`);
    body.push(md ? markdownToHtml(md) : `<p class="empty">${escapeHtml(labels.noSummary)}</p>`);
  }
  if (scope !== 'summary') {
    const segs = orderedSegments(input.transcripts);
    body.push(`<h2>${escapeHtml(labels.transcript)}</h2>`);
    if (segs.length === 0) {
      body.push(`<p class="empty">${escapeHtml(labels.noTranscript)}</p>`);
    } else {
      for (const s of segs) {
        const ts = stampOf(s);
        const who = speakerOf(input, s);
        body.push(
          `<p class="seg">${ts ? `<span class="ts">${escapeHtml(ts)}</span>` : ''}${
            who ? `<span class="who">${escapeHtml(who)}</span>` : ''
          }${escapeHtml(s.text.trim())}</p>`,
        );
      }
    }
  }

  return `<!DOCTYPE html>
<html lang="${escapeHtml(lang)}">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>${escapeHtml(title)}</title>
<style>${HTML_STYLE}</style>
</head>
<body>
${body.join('\n')}
</body>
</html>
`;
}

// ---------------------------------------------------------------------------
// Plain text document
// ---------------------------------------------------------------------------

export function buildMeetingText(
  input: MeetingExportInput,
  scope: ExportScope = 'both',
  labels: ExportLabels = DEFAULT_EXPORT_LABELS,
): string {
  const title = input.meeting.title?.trim() || labels.untitled;
  const out: string[] = [title, '='.repeat(Math.max(8, Math.min(60, title.length * 2)))];
  out.push(meetingMetaPairs(input, labels).map(([k, v]) => `${k}: ${v}`).join('\n'));

  if (scope !== 'transcript') {
    const md = summaryToMarkdown(input.summary);
    out.push(`\n【${labels.summary}】\n`);
    out.push(md ? markdownToPlainText(md).trimEnd() : labels.noSummary);
  }
  if (scope !== 'summary') {
    const segs = orderedSegments(input.transcripts);
    out.push(`\n【${labels.transcript}】\n`);
    out.push(
      segs.length === 0
        ? labels.noTranscript
        : segs
            .map((s) => {
              const head = [stampOf(s), speakerOf(input, s)].filter(Boolean).join(' ');
              return head ? `[${head}] ${s.text.trim()}` : s.text.trim();
            })
            .join('\n'),
    );
  }
  return out.join('\n') + '\n';
}

// ---------------------------------------------------------------------------
// Subtitles
// ---------------------------------------------------------------------------

function subtitleTime(seconds: number, separator: ',' | '.'): string {
  const ms = Math.max(0, Math.round(seconds * 1000));
  const h = Math.floor(ms / 3_600_000);
  const m = Math.floor((ms % 3_600_000) / 60_000);
  const s = Math.floor((ms % 60_000) / 1000);
  const rest = ms % 1000;
  const p2 = (n: number) => String(n).padStart(2, '0');
  return `${p2(h)}:${p2(m)}:${p2(s)}${separator}${String(rest).padStart(3, '0')}`;
}

/**
 * Subtitle cues need start and end times. Segments without a start time are
 * skipped; a missing or inverted end is closed at the next cue (max 6 s).
 */
export function buildSubtitles(input: MeetingExportInput, format: 'srt' | 'vtt'): string {
  const segs = orderedSegments(input.transcripts).filter(
    (s) => typeof s.audio_start_time === 'number' && Number.isFinite(s.audio_start_time),
  );
  const sep = format === 'srt' ? ',' : '.';
  const cues = segs.map((s, idx) => {
    const start = s.audio_start_time as number;
    const nextStart = segs[idx + 1]?.audio_start_time;
    let end = typeof s.audio_end_time === 'number' && s.audio_end_time > start ? s.audio_end_time : NaN;
    if (!Number.isFinite(end)) {
      end = typeof nextStart === 'number' && nextStart > start ? Math.min(nextStart, start + 6) : start + 3;
    }
    const who = speakerOf(input, s);
    const text = who ? `${who}: ${s.text.trim()}` : s.text.trim();
    const timing = `${subtitleTime(start, sep)} --> ${subtitleTime(end, sep)}`;
    return format === 'srt' ? `${idx + 1}\n${timing}\n${text}` : `${timing}\n${text}`;
  });
  if (format === 'vtt') return `WEBVTT\n\n${cues.join('\n\n')}\n`;
  return cues.join('\n\n') + (cues.length ? '\n' : '');
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

export function buildMeetingJson(input: MeetingExportInput, scope: ExportScope = 'both'): string {
  const doc: Record<string, unknown> = {
    format: 'meetily-export',
    version: 1,
    meeting: {
      id: input.meeting.id,
      title: input.meeting.title,
      created_at: input.meeting.created_at,
      updated_at: input.meeting.updated_at ?? null,
      duration_seconds: meetingMetaDuration(input),
    },
  };
  if (scope !== 'transcript') doc.summary_markdown = summaryToMarkdown(input.summary) || null;
  if (scope !== 'summary') {
    doc.transcript = orderedSegments(input.transcripts).map((s) => ({
      start: s.audio_start_time ?? null,
      end: s.audio_end_time ?? null,
      speaker: s.speaker ?? null,
      speaker_label: s.speaker ? speakerOf(input, s) : null,
      text: s.text.trim(),
    }));
  }
  return JSON.stringify(doc, null, 2) + '\n';
}

function meetingMetaDuration(input: MeetingExportInput): number | null {
  let max = 0;
  for (const s of input.transcripts ?? []) {
    const end = s.audio_end_time ?? s.audio_start_time;
    if (typeof end === 'number' && end > max) max = end;
  }
  return max > 0 ? Math.round(max * 10) / 10 : null;
}

// ---------------------------------------------------------------------------
// Word (.docx) — WordprocessingML parts; the Rust side zips them.
// ---------------------------------------------------------------------------

export function escapeXml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    // Characters XML 1.0 forbids would make Word refuse the whole file.
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F]/g, '');
}

function wRun(run: InlineRun, extra = ''): string {
  const props =
    (run.bold ? '<w:b/>' : '') +
    (run.italic ? '<w:i/>' : '') +
    (run.code ? '<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/><w:shd w:val="clear" w:fill="F3F4F6"/>' : '') +
    extra;
  return `<w:r>${props ? `<w:rPr>${props}</w:rPr>` : ''}<w:t xml:space="preserve">${escapeXml(run.text)}</w:t></w:r>`;
}

function wPara(runs: string, pPr = ''): string {
  return `<w:p>${pPr ? `<w:pPr>${pPr}</w:pPr>` : ''}${runs}</w:p>`;
}

function wInline(text: string, extra = ''): string {
  return parseInline(text)
    .map((r) => wRun(r, extra))
    .join('');
}

function docxBlocks(markdown: string): string[] {
  const out: string[] = [];
  for (const block of parseMarkdownBlocks(markdown)) {
    switch (block.type) {
      case 'heading': {
        // The document title is Heading1 and sections are Heading2, so summary
        // headings start at Heading2.
        const level = Math.min(block.level + 1, 4);
        out.push(wPara(wInline(block.text), `<w:pStyle w:val="Heading${level}"/>`));
        break;
      }
      case 'paragraph':
        out.push(wPara(wInline(block.text)));
        break;
      case 'quote':
        out.push(wPara(wInline(block.text, '<w:color w:val="4B5563"/>'), '<w:ind w:left="360"/>'));
        break;
      case 'code':
        for (const line of block.text.split('\n')) {
          out.push(wPara(wRun({ text: line || ' ', code: true })));
        }
        break;
      case 'hr':
        out.push(wPara('', '<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="D1D5DB"/></w:pBdr>'));
        break;
      case 'list':
        block.items.forEach((item, idx) => {
          const marker =
            item.checked !== undefined ? (item.checked ? '☑' : '☐') : block.ordered ? `${idx + 1}.` : '•';
          const left = 360 + item.depth * 360;
          out.push(
            wPara(
              wRun({ text: `${marker}\t` }) + wInline(item.text),
              `<w:tabs><w:tab w:val="left" w:pos="${left}"/></w:tabs><w:ind w:left="${left}" w:hanging="300"/>`,
            ),
          );
        });
        break;
      case 'table': {
        const cols = Math.max(...block.rows.map((r) => r.length));
        const rows = block.rows
          .map((row, r) => {
            const cells = Array.from({ length: cols }, (_, c) => {
              const text = row[c] ?? '';
              const shade = r === 0 ? '<w:shd w:val="clear" w:fill="F3F4F6"/>' : '';
              return `<w:tc><w:tcPr>${shade}</w:tcPr>${wPara(wInline(text, r === 0 ? '<w:b/>' : ''))}</w:tc>`;
            }).join('');
            return `<w:tr>${cells}</w:tr>`;
          })
          .join('');
        const border = (side: string) => `<w:${side} w:val="single" w:sz="4" w:space="0" w:color="D1D5DB"/>`;
        out.push(
          `<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblBorders>${['top', 'left', 'bottom', 'right', 'insideH', 'insideV']
            .map(border)
            .join('')}</w:tblBorders></w:tblPr>${rows}</w:tbl>`,
        );
        out.push(wPara(''));
        break;
      }
    }
  }
  return out;
}

const DOCX_STYLES = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults>
    <w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Microsoft YaHei" w:cs="Calibri"/><w:sz w:val="22"/><w:lang w:val="en-US" w:eastAsia="zh-CN"/></w:rPr></w:rPrDefault>
    <w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="300" w:lineRule="auto"/></w:pPr></w:pPrDefault>
  </w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="80"/></w:pPr><w:rPr><w:b/><w:sz w:val="40"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="320" w:after="120"/><w:pBdr><w:bottom w:val="single" w:sz="4" w:space="2" w:color="E5E7EB"/></w:pBdr><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:color w:val="1F2937"/><w:sz w:val="30"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:outlineLvl w:val="3"/></w:pPr><w:rPr><w:b/><w:sz w:val="23"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Meta"><w:name w:val="Meta"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="40"/></w:pPr><w:rPr><w:color w:val="6B7280"/><w:sz w:val="19"/></w:rPr></w:style>
</w:styles>`;

const DOCX_CONTENT_TYPES = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>`;

const DOCX_RELS = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>`;

const DOCX_DOC_RELS = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>`;

export function buildDocxParts(
  input: MeetingExportInput,
  scope: ExportScope = 'both',
  labels: ExportLabels = DEFAULT_EXPORT_LABELS,
): Array<{ path: string; content: string }> {
  const title = input.meeting.title?.trim() || labels.untitled;
  const body: string[] = [wPara(wRun({ text: title }), '<w:pStyle w:val="Title"/>')];
  for (const [k, v] of meetingMetaPairs(input, labels)) {
    body.push(wPara(wRun({ text: `${k}: `, bold: true }) + wRun({ text: v }), '<w:pStyle w:val="Meta"/>'));
  }

  if (scope !== 'transcript') {
    body.push(wPara(wRun({ text: labels.summary }), '<w:pStyle w:val="Heading2"/>'));
    const md = summaryToMarkdown(input.summary);
    if (md) body.push(...docxBlocks(md));
    else body.push(wPara(wRun({ text: labels.noSummary, italic: true })));
  }

  if (scope !== 'summary') {
    body.push(wPara(wRun({ text: labels.transcript }), '<w:pStyle w:val="Heading2"/>'));
    const segs = orderedSegments(input.transcripts);
    if (segs.length === 0) {
      body.push(wPara(wRun({ text: labels.noTranscript, italic: true })));
    }
    for (const s of segs) {
      const ts = stampOf(s);
      const who = speakerOf(input, s);
      const head =
        (ts ? wRun({ text: `${ts}  ` }, '<w:color w:val="9CA3AF"/><w:sz w:val="18"/>') : '') +
        (who ? wRun({ text: `${who}  `, bold: true }, '<w:color w:val="2563EB"/>') : '');
      body.push(wPara(head + wRun({ text: s.text.trim() })));
    }
  }

  const document = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<w:body>
${body.join('\n')}
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1304" w:right="1247" w:bottom="1304" w:left="1247" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>
</w:body>
</w:document>`;

  const now = new Date().toISOString().replace(/\.\d+Z$/, 'Z');
  const core = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:title>${escapeXml(title)}</dc:title><dc:creator>Meetily</dc:creator>
<dcterms:created xsi:type="dcterms:W3CDTF">${now}</dcterms:created>
</cp:coreProperties>`;

  return [
    { path: '[Content_Types].xml', content: DOCX_CONTENT_TYPES },
    { path: '_rels/.rels', content: DOCX_RELS },
    { path: 'word/document.xml', content: document },
    { path: 'word/styles.xml', content: DOCX_STYLES },
    { path: 'word/_rels/document.xml.rels', content: DOCX_DOC_RELS },
    { path: 'docProps/core.xml', content: core },
  ];
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/** Filesystem-safe base name (no extension). */
export function exportBaseName(meeting: { id: string; title?: string | null }): string {
  const raw = meeting.title?.trim() || meeting.id || 'meeting';
  const safe = raw
    .replace(/[\\/:*?"<>|\u0000-\u001F]/g, '-')
    .replace(/\s+/g, ' ')
    .replace(/^[.\s]+|[.\s]+$/g, '')
    .slice(0, 80);
  return safe || 'meeting';
}

export function exportFileNameFor(
  meeting: { id: string; title?: string | null },
  format: ExportFormat,
  scope: ExportScope,
): string {
  const suffix = formatUsesScope(format) && scope !== 'both' ? `-${scope}` : '';
  return `${exportBaseName(meeting)}${suffix}.${format}`;
}

export function buildExportFile(
  input: MeetingExportInput,
  format: ExportFormat,
  scope: ExportScope = 'both',
  labels: ExportLabels = DEFAULT_EXPORT_LABELS,
  lang = 'zh-CN',
): ExportFilePayload {
  const name = exportFileNameFor(input.meeting, format, scope);
  switch (format) {
    case 'docx':
      return { name, kind: 'docx', parts: buildDocxParts(input, scope, labels) };
    case 'md':
      return { name, kind: 'text', text: buildMeetingMarkdown(input, scope, labels) };
    case 'txt':
      return { name, kind: 'text', text: buildMeetingText(input, scope, labels) };
    case 'html':
      return { name, kind: 'text', text: buildMeetingHtml(input, scope, labels, lang) };
    case 'srt':
    case 'vtt':
      return { name, kind: 'text', text: buildSubtitles(input, format) };
    case 'json':
      return { name, kind: 'text', text: buildMeetingJson(input, scope) };
  }
}
