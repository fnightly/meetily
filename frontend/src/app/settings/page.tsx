'use client';

import React, { Suspense, useState, useEffect } from 'react';
import { useSearchParams } from 'next/navigation';
import { Settings2, Mic, Database as DatabaseIcon, SparkleIcon, FlaskConical, BookA, Archive, Share2 } from 'lucide-react';
import { configService } from '@/services/configService';
import { TranscriptSettings } from '@/components/TranscriptSettings';
import { RecordingSettings } from '@/components/RecordingSettings';
import { PreferenceSettings } from '@/components/PreferenceSettings';
import { SummaryModelSettings } from '@/components/SummaryModelSettings';
import { BetaSettings } from '@/components/BetaSettings';
import { useConfig } from '@/contexts/ConfigContext';
import { DictionarySettings } from '@/components/DictionarySettings';
import { BackupSettings } from '@/components/BackupSettings';
import { SyncSettingsPanel } from '@/components/SyncSettingsPanel';
import { SpeakerSeparationSettings } from '@/components/SpeakerSeparationSettings';
import { useI18n } from '@/contexts/I18nContext';
import { LOCALES, LOCALE_LABELS, type Locale } from '@/lib/i18n';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { PageHeader } from '@/components/layout/PageHeader';

// Tabs configuration (constant). Labels are translation keys, resolved at render
// so the tab bar follows the chosen interface language.
const TABS = [
  { value: 'general', labelKey: 'settings.tabs.general', icon: Settings2 },
  { value: 'recording', labelKey: 'settings.tabs.recording', icon: Mic },
  { value: 'dictionary', labelKey: 'settings.tabs.dictionary', icon: BookA },
  { value: 'Transcriptionmodels', labelKey: 'settings.tabs.transcription', icon: DatabaseIcon },
  { value: 'summaryModels', labelKey: 'settings.tabs.summary', icon: SparkleIcon },
  { value: 'sync', labelKey: 'sync.tab', icon: Share2 },
  { value: 'data', labelKey: 'settings.tabs.data', icon: Archive },
  { value: 'beta', labelKey: 'settings.tabs.beta', icon: FlaskConical }
] as const;

function SettingsContent() {
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  const { t, locale, setLocale } = useI18n();
  const searchParams = useSearchParams();

  // Animation state for tabs
  const [activeTab, setActiveTab] = useState('general');

  // Deep link from the sync dialog's "Open settings" button, so an unconfigured
  // Notion target lands on the panel that fixes it instead of on General. Also
  // follows later in-app query changes while the page stays mounted.
  const requestedTab = searchParams.get('tab');
  useEffect(() => {
    if (requestedTab && TABS.some((tab) => tab.value === requestedTab)) {
      setActiveTab(requestedTab);
    }
  }, [requestedTab]);

  // Load saved transcript configuration on mount
  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = await configService.getTranscriptConfig();
        if (config) {
          setTranscriptModelConfig({
            provider: config.provider || 'localWhisper',
            model: config.model || 'large-v3',
            apiKey: config.apiKey || null
          });
        }
      } catch (error) {
        console.error('Failed to load transcript config:', error);
      }
    };
    loadTranscriptConfig();
  }, [setTranscriptModelConfig]);

  const activeMeta = TABS.find((tab) => tab.value === activeTab) ?? TABS[0];

  return (
    <div className="h-screen bg-background flex flex-col">
      <PageHeader icon={<Settings2 />} title={t('settings.title')} description={t('settings.subtitle')} />

      <div className="flex-1 overflow-y-auto custom-scrollbar">
        <div className="mx-auto max-w-6xl px-8 py-6">
          <Tabs value={activeTab} onValueChange={setActiveTab} orientation="vertical" className="flex gap-8 items-start">
            <TabsList className="sticky top-0 flex h-auto w-52 flex-shrink-0 flex-col items-stretch gap-0.5 rounded-none bg-transparent p-0">
              {TABS.map((tab) => {
                const Icon = tab.icon;
                return (
                  <TabsTrigger
                    key={tab.value}
                    value={tab.value}
                    className="nav-item justify-start rounded-lg data-[state=active]:bg-white data-[state=active]:text-blue-700 data-[state=active]:shadow-[0_1px_2px_rgba(15,23,42,0.06)] data-[state=active]:ring-1 data-[state=active]:ring-slate-200"
                  >
                    <Icon className="w-4 h-4" />
                    {t(tab.labelKey)}
                  </TabsTrigger>
                );
              })}
            </TabsList>

            <div className="min-w-0 flex-1 max-w-3xl">
              <div className="mb-5">
                <h2 className="text-lg font-semibold text-slate-900">{t(activeMeta.labelKey)}</h2>
                <p className="mt-0.5 text-sm text-slate-500">{t(`settings.tabDesc.${activeMeta.value}`)}</p>
              </div>
            <TabsContent value="general" className="mt-0">
              {/* Interface language first: it changes how everything below reads,
                  and it is the one setting a user who cannot read the current
                  language needs to find immediately. */}
              <div className="surface-card p-5 mb-4">
                <div className="flex items-center justify-between gap-4">
                  <div>
                    <div className="font-semibold text-slate-900">{t('settings.language')}</div>
                    <div className="text-sm text-slate-500 mt-1">
                      {t('settings.language.description')}
                    </div>
                  </div>
                  <select
                    aria-label={t('settings.language')}
                    value={locale}
                    onChange={(event) => setLocale(event.target.value as Locale)}
                    className="shrink-0 h-9 px-3 text-sm border border-slate-300 rounded-lg bg-white focus:outline-none focus:ring-2 focus:ring-blue-500/30"
                  >
                    {LOCALES.map((value) => (
                      <option key={value} value={value}>
                        {LOCALE_LABELS[value]}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <PreferenceSettings />
            </TabsContent>
            <TabsContent value="recording" className="mt-0">
              <RecordingSettings />
            </TabsContent>
            <TabsContent value="dictionary" className="mt-0">
              <DictionarySettings />
            </TabsContent>
            <TabsContent value="Transcriptionmodels" className="mt-0">
              <TranscriptSettings
                transcriptModelConfig={transcriptModelConfig}
                setTranscriptModelConfig={setTranscriptModelConfig}
              />
              {/* Speaker separation belongs with transcription: it annotates the
                  transcript rather than producing one. */}
              <div className="mt-8 pt-6 border-t">
                <SpeakerSeparationSettings />
              </div>
            </TabsContent>
            <TabsContent value="summaryModels" className="mt-0">
              <SummaryModelSettings />
            </TabsContent>
            <TabsContent value="sync" className="mt-0">
              <SyncSettingsPanel />
            </TabsContent>
            <TabsContent value="data" className="mt-0">
              <BackupSettings />
            </TabsContent>
            <TabsContent value="beta" className="mt-0">
              <BetaSettings />
            </TabsContent>
            </div>
          </Tabs>
        </div>
      </div>
    </div>
  );
}

export default function SettingsPage() {
  // `useSearchParams` must sit inside a Suspense boundary for static export.
  return (
    <Suspense fallback={null}>
      <SettingsContent />
    </Suspense>
  );
}
