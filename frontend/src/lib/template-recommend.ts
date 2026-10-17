/**
 * Suggest a summary template from what is known about a meeting.
 *
 * Only the title and the opening of the transcript are used — cheap signals
 * that are available before any LLM call. Rules are ordered from the most
 * specific scene to the generic default.
 */

interface Rule {
  id: string;
  pattern: RegExp;
}

const RULES: Rule[] = [
  { id: 'zh_investment_committee', pattern: /(投决|投委|投资决策|IC\s*会|立项会|过会)/i },
  { id: 'zh_project_pitch', pattern: /(路演|尽调|尽职调查|BP|融资|估值|创始人|pitch|due\s*diligence)/i },
  { id: 'zh_job_interview', pattern: /(面试|候选人|interview|简历|offer)/i },
  { id: 'zh_customer_interview', pattern: /(客户访谈|用户访谈|访谈|调研|客户拜访|customer\s*interview|user\s*research)/i },
  { id: 'zh_weekly_meeting', pattern: /(周会|例会|周报|weekly|周例会|双周会)/i },
  { id: 'daily_standup', pattern: /(站会|standup|stand-up|晨会|daily)/i },
  { id: 'retrospective', pattern: /(复盘|回顾|retro)/i },
  { id: 'sales_marketing_client_call', pattern: /(销售|商务|sales|client\s*call|报价)/i },
  { id: 'project_sync', pattern: /(项目同步|项目进度|sync|对齐)/i },
];

export function recommendTemplate(
  title: string | null | undefined,
  transcriptSample: string | null | undefined,
  availableIds: string[],
): string | null {
  const available = new Set(availableIds);
  const haystacks = [title ?? '', (transcriptSample ?? '').slice(0, 1500)];
  for (const text of haystacks) {
    if (!text.trim()) continue;
    for (const rule of RULES) {
      if (available.has(rule.id) && rule.pattern.test(text)) return rule.id;
    }
  }
  return available.has('standard_meeting') ? 'standard_meeting' : null;
}

/** Built-in template ids whose names are shown translated. */
export const BUILTIN_TEMPLATE_KEYS: Record<string, string> = {
  standard_meeting: 'tpl.standard_meeting',
  daily_standup: 'tpl.daily_standup',
  project_sync: 'tpl.project_sync',
  retrospective: 'tpl.retrospective',
  sales_marketing_client_call: 'tpl.sales_marketing_client_call',
  psychiatric_session: 'tpl.psychiatric_session',
};
