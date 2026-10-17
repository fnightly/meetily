import { describe, expect, test } from 'bun:test';
import {
  buildDocxParts,
  buildExportFile,
  buildMeetingHtml,
  buildMeetingJson,
  buildMeetingText,
  buildSubtitles,
  escapeXml,
  exportBaseName,
  exportFileNameFor,
  markdownToHtml,
  markdownToPlainText,
  parseInline,
  parseMarkdownBlocks,
} from '../../src/lib/export-formats';
import { buildMeetingMarkdown, type ExportLabels } from '../../src/lib/meeting-export';

const meeting = { id: 'm-1', title: '产品周会 Q3', created_at: '2026-02-03T10:00:00.000Z' };
const summary = {
  markdown: [
    '## 关键结论',
    '- 上线时间定为 **3 月 15 日**',
    '- [x] 完成 *灰度* 方案',
    '  - 子项 `beta`',
    '',
    '| 负责人 | 事项 |',
    '| --- | --- |',
    '| 张三 | 接口 <联调> |',
    '',
    '普通段落 & 结束',
  ].join('\n'),
} as any;
const transcripts = [
  { text: '大家好', audio_start_time: 0, audio_end_time: 2.5, speaker: 'speaker_1' },
  { text: '我们开始吧', audio_start_time: 2.5, speaker: 'speaker_2' },
  { text: '   ', audio_start_time: 4 },
  { text: '好的', audio_start_time: 3725.2, audio_end_time: 3727 },
];
const input = { meeting, summary, transcripts, speakerLabel: (raw: string) => raw.replace('speaker_', '说话人 ') };
const zh: ExportLabels = {
  untitled: '未命名会议',
  date: '日期',
  meetingId: '会议 ID',
  segments: '片段数',
  duration: '时长',
  summary: '会议纪要',
  transcript: '转写全文',
  noSummary: '暂无纪要',
  noTranscript: '暂无转写',
};

describe('markdown parsing', () => {
  test('recognises headings, lists, tasks, tables and paragraphs', () => {
    const blocks = parseMarkdownBlocks(summary.markdown);
    expect(blocks.map((b) => b.type)).toEqual(['heading', 'list', 'table', 'paragraph']);
    const list = blocks[1] as any;
    expect(list.items).toHaveLength(3);
    expect(list.items[1]).toEqual({ text: '完成 *灰度* 方案', depth: 0, checked: true });
    expect(list.items[2].depth).toBe(1);
    expect((blocks[2] as any).rows).toEqual([
      ['负责人', '事项'],
      ['张三', '接口 <联调>'],
    ]);
  });

  test('inline runs carry bold / italic / code and keep link targets', () => {
    expect(parseInline('a **b** *c* `d`')).toEqual([
      { text: 'a ' },
      { text: 'b', bold: true },
      { text: ' ' },
      { text: 'c', italic: true },
      { text: ' ' },
      { text: 'd', code: true },
    ]);
    expect(parseInline('[文档](https://x.y)').map((r) => r.text).join('')).toBe('文档 (https://x.y)');
  });

  test('code fences are kept verbatim', () => {
    const blocks = parseMarkdownBlocks('```\n# not a heading\n```');
    expect(blocks).toEqual([{ type: 'code', text: '# not a heading' }]);
  });
});

describe('plain text and html', () => {
  test('plain text strips markup', () => {
    const text = markdownToPlainText(summary.markdown);
    expect(text).toContain('• 上线时间定为 3 月 15 日');
    expect(text).toContain('[x] 完成 灰度 方案');
    expect(text).not.toContain('**');
  });

  test('html escapes content', () => {
    const html = markdownToHtml(summary.markdown);
    expect(html).toContain('<strong>3 月 15 日</strong>');
    expect(html).toContain('接口 &lt;联调&gt;');
    expect(html).toContain('普通段落 &amp; 结束');
    expect(html).not.toContain('<联调>');
  });

  test('html document is self-contained and localized', () => {
    const doc = buildMeetingHtml(input, 'both', zh);
    expect(doc.startsWith('<!DOCTYPE html>')).toBe(true);
    expect(doc).toContain('<h2>会议纪要</h2>');
    expect(doc).toContain('<span class="who">说话人 1</span>');
    expect(doc).toContain('@media print');
  });

  test('text document follows scope and labels', () => {
    const t = buildMeetingText(input, 'transcript', zh);
    expect(t).not.toContain('会议纪要');
    expect(t).toContain('【转写全文】');
    expect(t).toContain('[00:00 说话人 1] 大家好');
    expect(t).toContain('[1:02:05] 好的');
  });

  test('markdown uses the given labels and reports duration', () => {
    const md = buildMeetingMarkdown(input, 'both', zh);
    expect(md).toContain('## 会议纪要');
    expect(md).toContain('## 转写全文');
    expect(md).toContain('- **时长:** 1:02:07');
  });
});

describe('subtitles', () => {
  test('srt numbers cues, uses comma millis and fills missing ends', () => {
    const srt = buildSubtitles(input, 'srt');
    const cues = srt.trim().split('\n\n');
    expect(cues).toHaveLength(3); // blank segment dropped
    expect(cues[0]).toBe('1\n00:00:00,000 --> 00:00:02,500\n说话人 1: 大家好');
    // no end: closed at the next cue, capped to 6 s
    expect(cues[1].split('\n')[1]).toBe('00:00:02,500 --> 00:00:08,500');
    expect(cues[2].split('\n')[1]).toBe('01:02:05,200 --> 01:02:07,000');
  });

  test('vtt has a header and dot millis', () => {
    const vtt = buildSubtitles(input, 'vtt');
    expect(vtt.startsWith('WEBVTT\n\n00:00:00.000 --> 00:00:02.500')).toBe(true);
  });
});

describe('json', () => {
  test('structured export follows scope', () => {
    const doc = JSON.parse(buildMeetingJson(input, 'transcript'));
    expect(doc.summary_markdown).toBeUndefined();
    expect(doc.transcript).toHaveLength(3);
    expect(doc.transcript[0]).toEqual({ start: 0, end: 2.5, speaker: 'speaker_1', speaker_label: '说话人 1', text: '大家好' });
    expect(doc.meeting.duration_seconds).toBe(3727);
  });
});

describe('docx', () => {
  test('produces the required parts with escaped, styled content', () => {
    const parts = buildDocxParts(input, 'both', zh);
    const paths = parts.map((p) => p.path);
    expect(paths).toContain('[Content_Types].xml');
    expect(paths).toContain('word/document.xml');
    expect(paths).toContain('word/styles.xml');
    expect(paths).toContain('word/_rels/document.xml.rels');
    const doc = parts.find((p) => p.path === 'word/document.xml')!.content;
    expect(doc).toContain('<w:pStyle w:val="Title"/>');
    expect(doc).toContain('<w:tbl>');
    expect(doc).toContain('接口 &lt;联调&gt;');
    expect(doc).toContain('<w:b/>');
    expect(doc).toContain('说话人 1');
    expect(doc).not.toMatch(/<联调>/);
  });

  test('xml escaping drops characters XML forbids', () => {
    expect(escapeXml('a\u0001b<c>&"')).toBe('ab&lt;c&gt;&amp;&quot;');
  });
});

describe('file naming and dispatch', () => {
  test('names are filesystem-safe and scoped', () => {
    expect(exportBaseName({ id: 'x', title: '周会: A/B?' })).toBe('周会- A-B-');
    expect(exportBaseName({ id: 'x', title: '  ' })).toBe('x');
    expect(exportFileNameFor(meeting, 'docx', 'summary')).toBe('产品周会 Q3-summary.docx');
    expect(exportFileNameFor(meeting, 'srt', 'summary')).toBe('产品周会 Q3.srt');
  });

  test('buildExportFile returns the right payload kind for every format', () => {
    for (const format of ['md', 'txt', 'html', 'srt', 'vtt', 'json'] as const) {
      const file = buildExportFile(input, format, 'both', zh);
      expect(file.kind).toBe('text');
      expect(file.name.endsWith(`.${format}`)).toBe(true);
    }
    const docx = buildExportFile(input, 'docx', 'both', zh);
    expect(docx.kind).toBe('docx');
  });
});
