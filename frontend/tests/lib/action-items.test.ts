import { describe, expect, test } from 'bun:test';
import { extractActionItems, isOverdue, parseBullet } from '../../src/lib/action-items';

describe('parseBullet', () => {
  test('reads a leading Chinese owner and a relative due date', () => {
    expect(parseBullet('- 张总：周五前发出 TS')).toEqual({ text: '周五前发出 TS', owner: '张总', due_date: null });
    const d = parseBullet('- 完成尽调报告（负责人：李四，截止 5月3日）');
    expect(d?.owner).toBe('李四');
  });

  test('reads English owner / by-phrases', () => {
    const d = parseBullet('* Send revised deck, Owner: Ann, by Friday');
    expect(d?.owner).toBe('Ann');
    expect(d?.due_date?.toLowerCase()).toBe('friday');
  });

  test('ignores empty placeholders', () => {
    expect(parseBullet('- 无')).toBeNull();
    expect(parseBullet('- N/A')).toBeNull();
  });
});

describe('extractActionItems', () => {
  test('reads a table under a Chinese heading using its header row', () => {
    const md = [
      '# 会议纪要', '## 关键决策', '- 同意进入尽调',
      '## 待办事项',
      '| 负责人 | 事项 | 截止时间 |',
      '| --- | --- | --- |',
      '| 王总 | 约见创始人 | 2025-07-01 |',
      '| **李四** | 整理财务模型 | 下周三 |',
      '## 风险', '- 估值偏高',
    ].join('\n');
    expect(extractActionItems(md)).toEqual([
      { text: '约见创始人', owner: '王总', due_date: '2025-07-01' },
      { text: '整理财务模型', owner: '李四', due_date: '下周三' },
    ]);
  });

  test('reads bullets under an English heading and stops at the next section', () => {
    const md = '## Action Items\n- Ann: send pricing by Friday\n- [ ] Book follow-up call\n## Notes\n- not a task';
    const items = extractActionItems(md);
    expect(items.map((i) => i.text)).toEqual(['send pricing by Friday', 'Book follow-up call']);
    expect(items[0].owner).toBe('Ann');
  });

  test('de-duplicates and returns nothing without a matching section', () => {
    expect(extractActionItems('## 待办\n- 发邮件\n- 发邮件')).toHaveLength(1);
    expect(extractActionItems('## 摘要\n- 讨论了产品')).toEqual([]);
    expect(extractActionItems(null)).toEqual([]);
  });
});

describe('isOverdue', () => {
  const now = new Date(2025, 5, 10);
  test('only ISO dates in the past and open items are overdue', () => {
    expect(isOverdue({ due_date: '2025-06-01', done: false }, now)).toBe(true);
    expect(isOverdue({ due_date: '2025-06-01', done: true }, now)).toBe(false);
    expect(isOverdue({ due_date: '2025-06-10', done: false }, now)).toBe(false);
    expect(isOverdue({ due_date: '周五', done: false }, now)).toBe(false);
  });
});
