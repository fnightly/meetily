'use client';

import React, { useState, useEffect, useLayoutEffect, useRef } from 'react';
import { ArrowLeft, Settings2, Mic, Database as DatabaseIcon, SparkleIcon, FlaskConical, BookA } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { motion } from 'framer-motion';
import { TranscriptSettings } from '@/components/TranscriptSettings';
import { RecordingSettings } from '@/components/RecordingSettings';
import { PreferenceSettings } from '@/components/PreferenceSettings';
import { SummaryModelSettings } from '@/components/SummaryModelSettings';
import { BetaSettings } from '@/components/BetaSettings';
import { useConfig } from '@/contexts/ConfigContext';
import { DictionarySettings } from '@/components/DictionarySettings';
import { SpeakerSeparationSettings } from '@/components/SpeakerSeparationSettings';
import { useI18n } from '@/contexts/I18nContext';
import { LOCALES, LOCALE_LABELS, type Locale } from '@/lib/i18n';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';

// Tabs configuration (constant). Labels are translation keys, resolved at render
// so the tab bar follows the chosen interface language.
const TABS = [
  { value: 'general', labelKey: 'settings.tabs.general', icon: Settings2 },
  { value: 'recording', labelKey: 'settings.tabs.recording', icon: Mic },
  { value: 'dictionary', labelKey: 'settings.tabs.dictionary', icon: BookA },
  { value: 'Transcriptionmodels', labelKey: 'settings.tabs.transcription', icon: DatabaseIcon },
  { value: 'summaryModels', labelKey: 'settings.tabs.summary', icon: SparkleIcon },
  { value: 'beta', labelKey: 'settings.tabs.beta', icon: FlaskConical }
] as const;

export default function SettingsPage() {
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  const { t, locale, setLocale } = useI18n();

  // Animation state for tabs
  const [activeTab, setActiveTab] = useState('general');
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [underlineStyle, setUnderlineStyle] = useState({ left: 0, width: 0 });

  // Load saved transcript configuration on mount
  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = await invoke('api_get_transcript_config') as any;
        if (config) {
          console.log('Loaded saved transcript config:', config);
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

  // Update underline position when active tab changes
  useLayoutEffect(() => {
    const activeIndex = TABS.findIndex(tab => tab.value === activeTab);
    const activeTabElement = tabRefs.current[activeIndex];

    if (activeTabElement) {
      const { offsetLeft, offsetWidth } = activeTabElement;
      setUnderlineStyle({ left: offsetLeft, width: offsetWidth });
    }
  }, [activeTab]);

  return (
    <div className="h-screen bg-gray-50 flex flex-col">
      {/* Fixed Header */}
      <div className="sticky top-0 z-10 bg-gray-50 border-b border-gray-200">
        <div className="max-w-6xl mx-auto px-8 py-6">
          <div className="flex items-center gap-4">
            <button
              onClick={() => router.back()}
              className="flex items-center gap-2 text-gray-600 hover:text-gray-900 transition-colors"
            >
              <ArrowLeft className="w-5 h-5" />
              <span>{t('common.back')}</span>
            </button>
            <h1 className="text-3xl font-bold">{t('settings.title')}</h1>
          </div>
        </div>
      </div>

      {/* Scrollable Content */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-6xl mx-auto p-8 pt-6">
          {/* Tabs */}
          <Tabs value={activeTab} onValueChange={setActiveTab}>
            <TabsList className="bg-transparent relative rounded-none border-b border-gray-200 p-0 h-auto">
              {TABS.map((tab, index) => {
                const Icon = tab.icon;
                return (
                  <TabsTrigger
                    key={tab.value}
                    value={tab.value}
                    ref={el => { tabRefs.current[index] = el }}
                    className="flex items-center gap-2 px-6 py-4 bg-transparent rounded-none border-0 data-[state=active]:bg-transparent data-[state=active]:text-blue-600 data-[state=active]:shadow-none text-gray-600 hover:text-gray-900 relative z-10"
                  >
                    <Icon className="w-4 h-4" />
                    {t(tab.labelKey)}
                  </TabsTrigger>
                );
              })}

              <motion.div
                className="absolute bottom-0 z-20 h-0.5 bg-blue-600"
                layoutId="underline"
                style={{ left: underlineStyle.left, width: underlineStyle.width }}
                transition={{ type: 'spring', stiffness: 400, damping: 40 }}
              />
            </TabsList>

            <TabsContent value="general">
              {/* Interface language first: it changes how everything below reads,
                  and it is the one setting a user who cannot read the current
                  language needs to find immediately. */}
              <div className="p-4 border rounded-lg mb-4">
                <div className="flex items-center justify-between gap-4">
                  <div>
                    <div className="font-medium">{t('settings.language')}</div>
                    <div className="text-sm text-gray-600 mt-1">
                      {t('settings.language.description')}
                    </div>
                  </div>
                  <select
                    aria-label={t('settings.language')}
                    value={locale}
                    onChange={(event) => setLocale(event.target.value as Locale)}
                    className="shrink-0 px-3 py-2 text-sm border border-gray-300 rounded-md bg-white"
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
            <TabsContent value="recording">
              <RecordingSettings />
            </TabsContent>
            <TabsContent value="dictionary" className="mt-6">
              <DictionarySettings />
            </TabsContent>
            <TabsContent value="Transcriptionmodels">
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
            <TabsContent value="summaryModels">
              <SummaryModelSettings />
            </TabsContent>
            <TabsContent value="beta" className="mt-6">
              <BetaSettings />
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </div>
  );
};
