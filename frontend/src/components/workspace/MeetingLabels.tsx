'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Briefcase, Hash, Plus, Tag, X } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';

export interface MeetingLabelsData {
  meeting_id: string;
  tags: string[];
  project: string | null;
}

export interface WorkspaceLabels {
  tags: Array<{ name: string; count: number }>;
  projects: Array<{ name: string; count: number }>;
}

export const LABELS_EVENT = 'meetily:labels-changed';

function announce(labels: MeetingLabelsData) {
  window.dispatchEvent(new CustomEvent(LABELS_EVENT, { detail: labels }));
}

/** Project + tags for one meeting, editable in a popover. */
export function MeetingLabels({ meetingId }: { meetingId: string }) {
  const { t } = useI18n();
  const [labels, setLabels] = useState<MeetingLabelsData>({ meeting_id: meetingId, tags: [], project: null });
  const [all, setAll] = useState<WorkspaceLabels>({ tags: [], projects: [] });
  const [open, setOpen] = useState(false);
  const [projectDraft, setProjectDraft] = useState('');
  const [tagDraft, setTagDraft] = useState('');

  useEffect(() => {
    invoke<MeetingLabelsData>('api_get_meeting_labels', { meetingId })
      .then((l) => { setLabels(l); setProjectDraft(l.project ?? ''); })
      .catch(() => {});
  }, [meetingId]);

  useEffect(() => {
    if (!open) return;
    invoke<WorkspaceLabels>('api_list_workspace_labels').then(setAll).catch(() => {});
  }, [open]);

  const saveTags = useCallback(async (tags: string[]) => {
    try {
      const next = await invoke<MeetingLabelsData>('api_set_meeting_tags', { meetingId, tags });
      setLabels(next);
      announce(next);
    } catch (e) {
      toast.error(t('labels.saveFailed'), { description: String(e) });
    }
  }, [meetingId, t]);

  const saveProject = useCallback(async (project: string) => {
    try {
      const next = await invoke<MeetingLabelsData>('api_set_meeting_project', { meetingId, project: project.trim() || null });
      setLabels(next);
      setProjectDraft(next.project ?? '');
      announce(next);
    } catch (e) {
      toast.error(t('labels.saveFailed'), { description: String(e) });
    }
  }, [meetingId, t]);

  const addTags = (raw: string) => {
    const parts = raw.split(/[,，、\s]+/).map((s) => s.replace(/^[#＃]/, '').trim()).filter(Boolean);
    if (parts.length === 0) return;
    void saveTags([...labels.tags, ...parts]);
    setTagDraft('');
  };

  const tagSuggestions = all.tags.map((x) => x.name).filter((n) => !labels.tags.includes(n)).slice(0, 10);
  const projectSuggestions = all.projects.map((x) => x.name).filter((n) => n !== labels.project).slice(0, 8);

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {labels.project && (
        <span className="chip bg-blue-50 text-blue-700"><Briefcase className="h-3 w-3" />{labels.project}</span>
      )}
      {labels.tags.map((tag) => (
        <span key={tag} className="chip bg-slate-100 text-slate-600"><Hash className="h-3 w-3 text-slate-400" />{tag}</span>
      ))}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            className="chip border border-dashed border-slate-300 bg-white text-slate-500 hover:border-blue-300 hover:text-blue-600"
            aria-label={t('labels.edit')}
          >
            {labels.project || labels.tags.length ? <Tag className="h-3 w-3" /> : <Plus className="h-3 w-3" />}
            {labels.project || labels.tags.length ? t('labels.edit') : t('labels.add')}
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-80 space-y-4 p-4">
          <div>
            <div className="eyebrow mb-1.5">{t('labels.project')}</div>
            <div className="flex gap-1.5">
              <input
                value={projectDraft}
                onChange={(e) => setProjectDraft(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') void saveProject(projectDraft); }}
                placeholder={t('labels.projectPlaceholder')}
                className="h-8 flex-1 rounded-md border border-slate-200 px-2 text-sm focus:border-blue-400 focus:outline-none focus:ring-2 focus:ring-blue-500/30"
              />
              <Button size="sm" variant="outline" onClick={() => void saveProject(projectDraft)}>{t('common.save')}</Button>
            </div>
            {projectSuggestions.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1">
                {projectSuggestions.map((p) => (
                  <button key={p} type="button" onClick={() => void saveProject(p)} className="chip bg-slate-100 text-slate-600 hover:bg-blue-50 hover:text-blue-700">
                    <Briefcase className="h-3 w-3" />{p}
                  </button>
                ))}
              </div>
            )}
          </div>
          <div>
            <div className="eyebrow mb-1.5">{t('labels.tags')}</div>
            <div className="flex flex-wrap gap-1 mb-2">
              {labels.tags.length === 0 && <span className="text-xs text-slate-400">{t('labels.noTags')}</span>}
              {labels.tags.map((tag) => (
                <span key={tag} className="chip bg-slate-100 text-slate-700">
                  #{tag}
                  <button type="button" aria-label={t('labels.removeTag', { tag })} onClick={() => void saveTags(labels.tags.filter((x) => x !== tag))} className="text-slate-400 hover:text-red-500">
                    <X className="h-3 w-3" />
                  </button>
                </span>
              ))}
            </div>
            <input
              value={tagDraft}
              onChange={(e) => setTagDraft(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addTags(tagDraft); } }}
              placeholder={t('labels.tagPlaceholder')}
              className="h-8 w-full rounded-md border border-slate-200 px-2 text-sm focus:border-blue-400 focus:outline-none focus:ring-2 focus:ring-blue-500/30"
            />
            {tagSuggestions.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1">
                {tagSuggestions.map((tag) => (
                  <button key={tag} type="button" onClick={() => void saveTags([...labels.tags, tag])} className="chip bg-slate-50 text-slate-500 hover:bg-blue-50 hover:text-blue-700">
                    #{tag}
                  </button>
                ))}
              </div>
            )}
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}
