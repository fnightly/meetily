import { describe, expect, test } from 'bun:test';
import { recommendTemplate } from '../../src/lib/template-recommend';

const ALL = ['standard_meeting', 'daily_standup', 'project_sync', 'retrospective', 'sales_marketing_client_call',
  'zh_weekly_meeting', 'zh_customer_interview', 'zh_job_interview', 'zh_investment_committee', 'zh_project_pitch'];

describe('recommendTemplate', () => {
  test('matches scenes from the title first', () => {
    expect(recommendTemplate('A 轮项目投决会', '', ALL)).toBe('zh_investment_committee');
    expect(recommendTemplate('某某科技路演', '', ALL)).toBe('zh_project_pitch');
    expect(recommendTemplate('产品经理面试', '', ALL)).toBe('zh_job_interview');
    expect(recommendTemplate('研发周会', '', ALL)).toBe('zh_weekly_meeting');
    expect(recommendTemplate('Q3 retro', '', ALL)).toBe('retrospective');
  });

  test('falls back to the transcript opening, then the standard template', () => {
    expect(recommendTemplate('会议 6月3日', '今天主要是做一个客户访谈，了解采购流程', ALL)).toBe('zh_customer_interview');
    expect(recommendTemplate('会议 6月3日', '大家好，我们开始吧', ALL)).toBe('standard_meeting');
  });

  test('never suggests a template that is not installed', () => {
    expect(recommendTemplate('投决会', '', ['standard_meeting'])).toBe('standard_meeting');
    expect(recommendTemplate('投决会', '', [])).toBeNull();
  });
});
