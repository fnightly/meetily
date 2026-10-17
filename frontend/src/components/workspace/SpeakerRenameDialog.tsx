'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { UserRound } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';
import { formatSpeakerLabel } from '@/lib/speaker-labels';
import { useSpeakerNames } from '@/lib/speaker-names';

/** Name one diarized speaker; the name is applied everywhere in this meeting. */
export function SpeakerRenameDialog({
  label,
  sample,
  onClose,
}: {
  label: string | null;
  /** A line this speaker said, so the user knows who it is. */
  sample?: string | null;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const { names, rename } = useSpeakerNames();
  const [draft, setDraft] = useState('');
  const [known, setKnown] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!label) return;
    setDraft(names[label] ?? '');
    invoke<string[]>('api_list_known_speaker_names').then(setKnown).catch(() => setKnown([]));
  }, [label, names]);

  const submit = async (value: string | null) => {
    if (!label) return;
    setSaving(true);
    try {
      await rename(label, value && value.trim() ? value.trim() : null);
      toast.success(value ? t('speakers.renamed', { name: value.trim() }) : t('speakers.cleared'));
      onClose();
    } catch (error) {
      toast.error(t('speakers.renameFailed'), { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const generic = label ? formatSpeakerLabel(label, t) : '';
  const suggestions = known.filter((n) => n !== draft.trim()).slice(0, 8);

  return (
    <Dialog open={label !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <UserRound className="h-4 w-4 text-blue-600" />
            {t('speakers.renameTitle', { label: generic })}
          </DialogTitle>
          <DialogDescription>{t('speakers.renameHint')}</DialogDescription>
        </DialogHeader>
        {sample && (
          <blockquote className="rounded-lg border-l-2 border-blue-300 bg-slate-50 px-3 py-2 text-xs leading-relaxed text-slate-600 line-clamp-3">
            {sample}
          </blockquote>
        )}
        <input
          autoFocus
          value={draft}
          maxLength={40}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void submit(draft); }}
          placeholder={t('speakers.namePlaceholder')}
          aria-label={t('speakers.namePlaceholder')}
          className="h-10 w-full rounded-lg border border-slate-200 px-3 text-sm focus:border-blue-400 focus:outline-none focus:ring-2 focus:ring-blue-500/30"
        />
        {suggestions.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            <span className="text-xs text-slate-400 mr-1 self-center">{t('speakers.recent')}</span>
            {suggestions.map((name) => (
              <button
                key={name}
                type="button"
                onClick={() => setDraft(name)}
                className="chip bg-slate-100 text-slate-700 hover:bg-blue-50 hover:text-blue-700"
              >
                {name}
              </button>
            ))}
          </div>
        )}
        <DialogFooter className="gap-2 sm:justify-between">
          {label && names[label] ? (
            <Button variant="ghost" size="sm" disabled={saving} onClick={() => void submit(null)}>
              {t('speakers.clear')}
            </Button>
          ) : <span />}
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={onClose}>{t('common.cancel')}</Button>
            <Button size="sm" disabled={saving || !draft.trim()} onClick={() => void submit(draft)}>{t('common.save')}</Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
