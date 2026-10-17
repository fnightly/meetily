'use client';

import { Suspense } from 'react';
import { MessageCircleQuestion } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { PageHeader } from '@/components/layout/PageHeader';
import { AskPanel } from '@/components/workspace/AskPanel';

function AskPageContent() {
  const { t } = useI18n();
  return (
    <div className="flex h-screen flex-col bg-background">
      <PageHeader icon={<MessageCircleQuestion />} title={t('ask.pageTitle')} description={t('ask.pageDesc')} maxWidthClass="max-w-3xl" />
      <div className="min-h-0 flex-1 overflow-hidden">
        <div className="mx-auto flex h-full w-full max-w-3xl flex-col px-8 py-6">
          <AskPanel />
        </div>
      </div>
    </div>
  );
}

export default function AskPage() {
  return (
    <Suspense fallback={null}>
      <AskPageContent />
    </Suspense>
  );
}
