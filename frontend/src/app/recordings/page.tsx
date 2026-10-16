'use client';

import { Suspense } from 'react';
import { AudioLines } from 'lucide-react';
import { RecordingsLibrary } from '@/components/RecordingsLibrary';
import { useI18n } from '@/contexts/I18nContext';

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
    <div className="h-full flex flex-col">
      <div className="flex-shrink-0 px-6 pt-6 pb-4 border-b">
        <div className="flex items-center gap-2">
          <AudioLines className="w-5 h-5 text-gray-600" />
          <h1 className="text-2xl font-bold text-gray-900">{t('recordingsPage.title')}</h1>
        </div>
        <p className="text-sm text-gray-600 mt-1">{t('recordingsPage.description')}</p>
      </div>

      <div className="flex-1 min-h-0 overflow-auto px-6 py-4">
        <RecordingsLibrary />
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
