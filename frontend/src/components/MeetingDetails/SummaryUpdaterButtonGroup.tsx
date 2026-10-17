"use client";
import { useI18n } from '@/contexts/I18nContext';

import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy, Save, Loader2 } from 'lucide-react';
import Analytics from '@/lib/analytics';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
}

export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onCopy,
}: SummaryUpdaterButtonGroupProps) {
  const { t } = useI18n();
  return (
    <ButtonGroup>
      {/* Save button */}
      <Button
        variant="outline"
        size="sm"
        className={isDirty ? 'border-emerald-300 bg-emerald-50 text-emerald-700 hover:bg-emerald-100' : ''}
        title={isSaving ? t('sumBtn.saving') : t('sumBtn.saveHint')}
        onClick={() => {
          Analytics.trackButtonClick('save_changes', 'meeting_details');
          onSave();
        }}
        disabled={isSaving}
      >
        {isSaving ? (
          <>
            <Loader2 className="animate-spin" />
            <span className="hidden @[40rem]:inline">{t('sumBtn.saving')}</span>
          </>
        ) : (
          <>
            <Save />
            <span className="hidden @[40rem]:inline">{t('common.save')}</span>
          </>
        )}
      </Button>

      {/* Copy button */}
      <Button
        variant="outline"
        size="sm"
        title={t('sumBtn.copyHint')}
        onClick={() => {
          Analytics.trackButtonClick('copy_summary', 'meeting_details');
          onCopy();
        }}
        className="cursor-pointer"
      >
        <Copy />
        <span className="hidden @[40rem]:inline">{t('transcriptActions.copyShort')}</span>
      </Button>

    </ButtonGroup>
  );
}
