/**
 * Pull follow-ups out of a generated summary.
 *
 * Summaries come from templates in both languages, so the parser looks for an
 * "action items"-like heading (待办事项 / 行动项 / 后续跟进 / Action Items /
 * Next Steps …) and reads either its Markdown table — using the header row to
 * find the owner, task and due columns — or its bullet list, where owners and
 * dates are recognised from common phrasings ("负责人：张三", "@王总",
 * "截止 5月3日", "Owner: Ann", "by Friday").
 */

export interface ActionItemDraft {
  text: string;
  owner: string | null;
  due_date: string | null;
}

export interface ActionItem {
  id: string;
  meeting_id: string | null;
  meeting_title: string | null;
  text: string;
  owner: string | null;
  due_date: string | null;
  done: boolean;
  source: 'summary' | 'manual' | string;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
}

const SECTION_RE = /(action\s*items?|next\s*steps?|follow[\s-]*ups?|to[\s-]*dos?|待办|行动项|后续跟进|跟进事项|下一步|后续计划|尽调计划|投资条件与待办|任务分配)/i;
const EMPTY_RE = /^(none|n\/a|无|暂无|未提及|没有|待定|none noted.*|—|-|–)$/i;
const OWNER_COL = /(owner|assignee|负责人|责任人|负责|执行人|跟进人)/i;
const TASK_COL = /(task|action|item|事项|任务|内容|行动|工作)/i;
const DUE_COL = /(due|deadline|date|截止|时间|日期|期限)/i;

function stripMd(text: string): string {
  return text
    .replace(/\*\*|__|`/g, '')
    .replace(/^\s*[-*+]\s*\[[ xX]\]\s*/, '')
    .replace(/^\s*(?:[-*+•]|\d+[.)、])\s*/, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function cleanCell(cell: string): string | null {
  const v = stripMd(cell);
  return v && !EMPTY_RE.test(v) ? v : null;
}

function splitRow(line: string): string[] {
  return line.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
}

const OWNER_PATTERNS: RegExp[] = [
  /[（(]\s*(?:负责人|责任人|负责|owner)\s*[:：]?\s*([^）)，,；;]{1,20})[）)]/i,
  /(?:负责人|责任人|owner|assignee)\s*[:：]\s*([^，,；;。)\]）]{1,20})/i,
  /@([\p{L}\p{N}_.-]{1,20})/u,
];
const DUE_PATTERNS: RegExp[] = [
  /[（(]\s*(?:截止|截至|due|deadline)\s*[:：]?\s*([^）)，,；;]{1,24})[）)]/i,
  /(?:截止(?:时间|日期)?|截至|due(?:\s*date)?|deadline)\s*[:：]?\s*([^，,；;。)\]）]{1,24})/i,
  /\bby\s+((?:mon|tues|wednes|thurs|fri|satur|sun)day|tomorrow|next\s+week|end\s+of\s+(?:day|week|month)|\d{1,2}\/\d{1,2})/i,
  /((?:\d{4}[-/年])?\d{1,2}[-/月]\d{1,2}日?(?:前|之前)?)/,
  /((?:本|下)周[一二三四五六日天]|明天|后天|月底|周末)(?:前|之前)?/,
];

/** Owner and due date recognised inside a free-text bullet. */
export function parseBullet(raw: string): ActionItemDraft | null {
  let text = stripMd(raw);
  if (!text || EMPTY_RE.test(text)) return null;
  let owner: string | null = null;
  let due: string | null = null;
  for (const re of OWNER_PATTERNS) {
    const m = re.exec(text);
    if (m) {
      owner = m[1].trim();
      if (m[0].startsWith('（') || m[0].startsWith('(')) text = text.replace(m[0], '').trim();
      break;
    }
  }
  // "张三：完成 BP 修改" — a short name before a colon at the start.
  if (!owner) {
    const lead = /^([\p{L}·]{1,8})\s*[:：]\s*(.+)$/u.exec(text);
    if (lead && !/(负责人|截止|备注|说明|note)/i.test(lead[1])) {
      owner = lead[1];
      text = lead[2].trim();
    }
  }
  for (const re of DUE_PATTERNS) {
    const m = re.exec(text);
    if (m) {
      due = m[1].trim();
      if (m[0].startsWith('（') || m[0].startsWith('(')) text = text.replace(m[0], '').trim();
      break;
    }
  }
  text = text.replace(/[，,；;]\s*$/, '').trim();
  return text ? { text, owner, due_date: due } : null;
}

/** Action items found in summary Markdown, in document order. */
export function extractActionItems(markdown: string | null | undefined): ActionItemDraft[] {
  if (!markdown) return [];
  const lines = markdown.split(/\r?\n/);
  const out: ActionItemDraft[] = [];
  let inSection = false;
  let sectionLevel = 0;
  let header: string[] | null = null;

  for (const line of lines) {
    const heading = /^(#{1,6})\s+(.*)$/.exec(line.trim());
    // Bold-only lines often act as headings in model output.
    const boldHeading = /^\*\*([^*]+)\*\*\s*[:：]?\s*$/.exec(line.trim());
    if (heading || boldHeading) {
      const level = heading ? heading[1].length : 6;
      const title = heading ? heading[2] : boldHeading![1];
      if (SECTION_RE.test(title)) {
        inSection = true;
        sectionLevel = level;
        header = null;
        continue;
      }
      if (inSection && level <= sectionLevel) inSection = false;
      continue;
    }
    if (!inSection) continue;
    const trimmed = line.trim();
    if (trimmed.startsWith('|')) {
      const cells = splitRow(trimmed);
      if (cells.every((c) => /^:?-{2,}:?$/.test(c) || c === '')) continue;
      if (!header) {
        header = cells;
        continue;
      }
      const col = (re: RegExp) => header!.findIndex((h) => re.test(stripMd(h)));
      const ownerIdx = col(OWNER_COL);
      let taskIdx = col(TASK_COL);
      const dueIdx = col(DUE_COL);
      if (taskIdx < 0 || taskIdx === ownerIdx || taskIdx === dueIdx) {
        taskIdx = cells.findIndex((_, i) => i !== ownerIdx && i !== dueIdx);
      }
      const text = cleanCell(cells[taskIdx] ?? '');
      if (!text) continue;
      out.push({
        text,
        owner: ownerIdx >= 0 ? cleanCell(cells[ownerIdx] ?? '') : null,
        due_date: dueIdx >= 0 ? cleanCell(cells[dueIdx] ?? '') : null,
      });
      continue;
    }
    header = null;
    if (/^\s*(?:[-*+•]|\d+[.)、])\s+/.test(line)) {
      const draft = parseBullet(line);
      if (draft) out.push(draft);
    }
  }
  // De-duplicate by normalized text.
  const seen = new Set<string>();
  return out.filter((d) => {
    const key = d.text.toLowerCase().replace(/[^\p{L}\p{N}]/gu, '');
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Overdue when the due text is an ISO-ish date before today. */
export function isOverdue(item: Pick<ActionItem, 'due_date' | 'done'>, now: Date = new Date()): boolean {
  if (item.done || !item.due_date) return false;
  const m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(item.due_date.trim());
  if (!m) return false;
  const due = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), 23, 59, 59);
  return due.getTime() < now.getTime();
}
