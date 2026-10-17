'use client';

import { Suspense, useEffect } from 'react';
import { ListChecks } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { PageHeader } from '@/components/layout/PageHeader';
import { ActionItemsPanel } from '@/components/workspace/ActionItemsPanel';
import { backfillActionItems } from '@/lib/action-items-sync';

function TasksPageContent() {
  const { t } = useI18n();
  useEffect(() => { void backfillActionItems(); }, []);
  return (
    <div className="flex h-screen flex-col bg-background">
      <PageHeader icon={<ListChecks />} title={t('tasks.pageTitle')} description={t('tasks.pageDesc')} maxWidthClass="max-w-4xl" />
      <div className="min-h-0 flex-1 overflow-hidden">
        <div className="mx-auto flex h-full w-full max-w-4xl flex-col px-8 py-6">
          <div className="surface-card flex min-h-0 flex-1 flex-col p-5">
            <ActionItemsPanel />
          </div>
        </div>
      </div>
    </div>
  );
}

export default function TasksPage() {
  return (
    <Suspense fallback={null}>
      <TasksPageContent />
    </Suspense>
  );
}
