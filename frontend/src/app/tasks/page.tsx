'use client';

import { Suspense } from 'react';
import { ListChecks } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { PageHeader } from '@/components/layout/PageHeader';
import { ActionItemsPanel } from '@/components/workspace/ActionItemsPanel';

function TasksPageContent() {
  const { t } = useI18n();
  // No backfill here: the one-per-session pass is scheduled from the app's
  // landing screen and runs after it goes idle. The panel paints from the stored
  // list straight away, so opening Tasks no longer waits on a library walk.
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
