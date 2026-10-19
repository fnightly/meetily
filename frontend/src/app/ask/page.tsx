'use client';

import { Suspense } from 'react';
import dynamic from 'next/dynamic';
import { MessageCircleQuestion } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { PageHeader } from '@/components/layout/PageHeader';

/**
 * AskPanel pulls in react-markdown and remark-gfm, which are only needed once an
 * answer exists. Loading it on demand lets the page header and composer shell
 * paint before that bundle is parsed, instead of blocking the route on it.
 */
const AskPanel = dynamic(() => import('@/components/workspace/AskPanel').then((mod) => mod.AskPanel), {
  ssr: false,
  loading: () => <AskPanelSkeleton />,
});

/** Same geometry as the loaded panel, so the swap does not shift the layout. */
function AskPanelSkeleton() {
  const { t } = useI18n();
  return (
    <div role="status" className="flex h-full min-h-0 flex-col" aria-busy="true" aria-label={t('common.loading')}>
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 text-center">
        <span className="flex h-11 w-11 animate-pulse items-center justify-center rounded-full bg-violet-50 text-violet-300">
          <MessageCircleQuestion className="h-5 w-5" />
        </span>
        <div className="h-3 w-40 animate-pulse rounded bg-slate-200" />
        <div className="h-2.5 w-64 max-w-full animate-pulse rounded bg-slate-100" />
      </div>
      <div className="pt-3">
        <div className="flex items-center gap-2 rounded-xl border border-slate-200 bg-white p-2 shadow-sm">
          <div className="h-4 w-4 flex-shrink-0 animate-pulse rounded bg-slate-100" />
          <div className="h-8 flex-1 animate-pulse rounded bg-slate-100" />
          <div className="h-9 w-9 flex-shrink-0 animate-pulse rounded-lg bg-slate-100" />
        </div>
      </div>
    </div>
  );
}

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
