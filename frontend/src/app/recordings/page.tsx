'use client';

import { Suspense } from 'react';
import { AudioLines } from 'lucide-react';
import { RecordingsLibrary } from '@/components/RecordingsLibrary';
import { useI18n } from '@/contexts/I18nContext';
import { PageHeader } from '@/components/layout/PageHeader';

/**
 * The recordings library as a top-level destination.
 *
 * It previously lived only inside a settings tab, which made saved audio feel
 * like a configuration detail rather than the app's own content. Recordings are a
 * first-class list — the same kind of thing as Meeting Notes — so they get their
 * own route and sidebar entry.
 *
 * Wrapped in `Suspense` because the app is statically exported and the library
 * reads Tauri commands on mount; the prerendered HTML is the fallback.
 */
function RecordingsPageContent() {
  const { t } = useI18n();

  return (
    <div className="h-screen flex flex-col bg-background">
      <PageHeader
        icon={<AudioLines />}
        title={t('recordingsPage.title')}
        description={t('recordingsPage.description')}
      />
      <div className="flex-1 min-h-0 overflow-auto custom-scrollbar">
        <div className="mx-auto w-full max-w-6xl px-8 py-6">
          <RecordingsLibrary />
        </div>
      </div>
    </div>
  );
}

export default function RecordingsPage() {
  return (
    <Suspense fallback={null}>
      <RecordingsPageContent />
    </Suspense>
  );
}
