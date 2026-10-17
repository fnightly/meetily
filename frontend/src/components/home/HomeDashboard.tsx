'use client';

import { useEffect, useMemo, useState } from 'react';
import { useRouter } from 'next/navigation';
import {
  ArrowRight, AudioLines, ChevronRight, Cpu, FileText, Languages, Mic, MonitorSpeaker,
  ShieldCheck, Sparkles, Upload, Library, Settings2, Cloud, ListChecks, MessageCircleQuestion,
} from 'lucide-react';
import { ActionItemsPanel } from '@/components/workspace/ActionItemsPanel';
import { backfillActionItems } from '@/lib/action-items-sync';
import { useI18n } from '@/contexts/I18nContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { usePrewarmSpeechModel } from '@/hooks/usePrewarmSpeechModel';
import { buildHomeOverview, greetingKey, splitDuration } from '@/lib/home-overview';
import { formatDuration, formatRecordedAt } from '@/lib/recordings';
import type { SidebarEntry } from '@/lib/sidebar-entries';

const PROVIDER_LABELS: Record<string, string> = {
  localWhisper: 'Whisper',
  parakeet: 'Parakeet',
  deepgram: 'Deepgram',
  elevenLabs: 'ElevenLabs',
  groq: 'Groq',
  openai: 'OpenAI',
  siliconflow: 'SiliconFlow',
};
const LOCAL_PROVIDERS = new Set(['localWhisper', 'parakeet']);

interface HomeDashboardProps {
  hasMicrophone: boolean;
  onStart: () => void;
  isStarting?: boolean;
}

/**
 * Idle home screen: one obvious way to start, the recording setup at a glance,
 * and the meetings that still need something (transcript or summary).
 */
export function HomeDashboard({ hasMicrophone, onStart, isStarting = false }: HomeDashboardProps) {
  const { t, locale } = useI18n();
  const router = useRouter();
  const { entries, setCurrentMeeting } = useSidebar();
  const { selectedDevices, transcriptModelConfig, selectedLanguage, betaFeatures } = useConfig();
  const { openImportDialog } = useImportDialog();
  const prewarm = usePrewarmSpeechModel();
  const [now, setNow] = useState(() => new Date());

  // Mine summaries written before follow-ups existed (once per session).
  useEffect(() => { void backfillActionItems(); }, []);

  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  const overview = useMemo(() => buildHomeOverview(entries, now), [entries, now]);
  const week = splitDuration(overview.weekSeconds);
  const provider = transcriptModelConfig?.provider ?? 'parakeet';
  const isLocal = LOCAL_PROVIDERS.has(provider);

  const languageLabel = useMemo(() => {
    if (!selectedLanguage || selectedLanguage.startsWith('auto')) return t('home.langAuto');
    try {
      return new Intl.DisplayNames([locale], { type: 'language' }).of(selectedLanguage) ?? selectedLanguage;
    } catch {
      return selectedLanguage;
    }
  }, [selectedLanguage, locale, t]);

  const dateLabel = useMemo(
    () => now.toLocaleDateString(locale, { month: 'long', day: 'numeric', weekday: 'long' }),
    [now, locale],
  );

  const openEntry = (entry: SidebarEntry) => {
    if (!entry.meetingId) {
      router.push('/recordings');
      return;
    }
    setCurrentMeeting({ id: entry.meetingId, title: entry.title });
    router.push(`/meeting-details?id=${encodeURIComponent(entry.meetingId)}`);
  };

  const setupItems = [
    {
      icon: Mic,
      label: t('home.setup.mic'),
      value: selectedDevices?.micDevice || t('home.setup.defaultDevice'),
    },
    {
      icon: MonitorSpeaker,
      label: t('home.setup.system'),
      value: selectedDevices?.systemDevice || t('home.setup.defaultDevice'),
    },
    {
      icon: isLocal ? Cpu : Cloud,
      label: t('home.setup.engine'),
      value: `${PROVIDER_LABELS[provider] ?? provider}${transcriptModelConfig?.model ? ` · ${transcriptModelConfig.model}` : ''}`,
    },
    { icon: Languages, label: t('home.setup.language'), value: languageLabel },
  ];

  const quickActions = [
    ...(betaFeatures.importAndRetranscribe
      ? [{ key: 'import', icon: Upload, title: t('home.quick.import'), hint: t('home.quick.importHint'), onClick: () => openImportDialog() }]
      : []),
    { key: 'library', icon: Library, title: t('home.quick.library'), hint: t('home.quick.libraryHint'), onClick: () => router.push('/recordings') },
    { key: 'settings', icon: Settings2, title: t('home.quick.settings'), hint: t('home.quick.settingsHint'), onClick: () => router.push('/settings') },
  ];

  return (
    <div className="h-full w-full overflow-y-auto custom-scrollbar">
      <div className="mx-auto w-full max-w-5xl px-8 pt-10 pb-16">
        {/* Greeting */}
        <header className="mb-6">
          <p className="text-sm text-slate-500">{dateLabel}</p>
          <h1 className="mt-1 text-[26px] font-semibold tracking-tight text-slate-900">
            {t(`home.greeting.${greetingKey(now)}`)}
          </h1>
        </header>

        {/* Hero: start recording */}
        <section className="surface-card relative overflow-hidden">
          <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_at_top_right,rgba(59,130,246,0.10),transparent_55%)]" />
          <div className="relative flex flex-col gap-8 p-7 md:flex-row md:items-center">
            <div className="flex-1 min-w-0">
              <h2 className="text-lg font-semibold text-slate-900">{t('home.hero.title')}</h2>
              <p className="mt-1 text-sm text-slate-500 leading-relaxed">{t('home.hero.subtitle')}</p>

              <dl className="mt-5 grid grid-cols-1 gap-2 sm:grid-cols-2">
                {setupItems.map(({ icon: Icon, label, value }) => (
                  <div key={label} className="flex items-center gap-2.5 rounded-lg bg-slate-50 px-3 py-2 min-w-0">
                    <Icon className="h-4 w-4 flex-shrink-0 text-slate-400" />
                    <dt className="flex-shrink-0 text-xs text-slate-500">{label}</dt>
                    <dd className="min-w-0 truncate text-xs font-medium text-slate-800" title={value}>{value}</dd>
                  </div>
                ))}
              </dl>
              <button
                type="button"
                onClick={() => router.push('/settings')}
                className="mt-3 inline-flex items-center gap-1 text-xs font-medium text-blue-600 hover:text-blue-700"
              >
                {t('home.setup.adjust')} <ChevronRight className="h-3 w-3" />
              </button>
            </div>

            <div className="flex flex-col items-center gap-3 md:pr-6">
              <div className="relative">
                <span className="absolute inset-0 rounded-full bg-red-400/40 animate-ring-breathe" aria-hidden />
                <button
                  type="button"
                  {...prewarm}
                  onClick={onStart}
                  disabled={!hasMicrophone || isStarting}
                  aria-label={t('nav.startRecording')}
                  className="relative flex h-24 w-24 items-center justify-center rounded-full bg-record text-white shadow-[0_10px_30px_-8px_rgba(239,68,68,0.6)] transition-transform hover:scale-[1.04] active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-4 focus-visible:ring-red-200"
                >
                  <Mic className="h-9 w-9" />
                </button>
              </div>
              <div className="text-center">
                <div className="text-sm font-semibold text-slate-900">
                  {isStarting ? t('home.hero.starting') : t('nav.startRecording')}
                </div>
                <div className="text-xs text-slate-400">
                  {hasMicrophone ? t('home.hero.ready') : t('home.hero.noMic')}
                </div>
              </div>
            </div>
          </div>
        </section>

        {/* Quick actions */}
        <section className={`mt-4 grid gap-3 ${quickActions.length === 3 ? 'sm:grid-cols-3' : 'sm:grid-cols-2'}`}>
          {quickActions.map(({ key, icon: Icon, title, hint, onClick }) => (
            <button
              key={key}
              type="button"
              onClick={onClick}
              className="surface-card surface-card-hover group flex items-center gap-3 p-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
            >
              <span className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-blue-50 text-blue-600">
                <Icon className="h-[18px] w-[18px]" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-semibold text-slate-900">{title}</span>
                <span className="block truncate text-xs text-slate-500">{hint}</span>
              </span>
              <ArrowRight className="h-4 w-4 text-slate-300 transition-transform group-hover:translate-x-0.5 group-hover:text-slate-500" />
            </button>
          ))}
        </section>

        <div className="mt-8 grid gap-6 lg:grid-cols-3">
          {/* Recent meetings */}
          <section className="lg:col-span-2">
            <div className="mb-3 flex items-center justify-between">
              <h3 className="text-sm font-semibold text-slate-900">{t('home.recent.title')}</h3>
              {overview.total > 0 && (
                <button
                  type="button"
                  onClick={() => router.push('/recordings')}
                  className="inline-flex items-center gap-1 text-xs font-medium text-slate-500 hover:text-slate-900"
                >
                  {t('home.recent.viewAll', { count: overview.total })} <ChevronRight className="h-3 w-3" />
                </button>
              )}
            </div>

            {overview.recent.length === 0 ? (
              <div className="surface-card flex flex-col items-center justify-center px-6 py-12 text-center">
                <span className="flex h-11 w-11 items-center justify-center rounded-full bg-slate-100 text-slate-400">
                  <AudioLines className="h-5 w-5" />
                </span>
                <p className="mt-3 text-sm font-medium text-slate-700">{t('home.recent.emptyTitle')}</p>
                <p className="mt-1 max-w-sm text-xs text-slate-500">{t('home.recent.emptyBody')}</p>
              </div>
            ) : (
              <ul className="surface-card divide-y divide-slate-100 overflow-hidden">
                {overview.recent.map((entry) => (
                  <li key={entry.key}>
                    <button
                      type="button"
                      onClick={() => openEntry(entry)}
                      className="group flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-slate-50 focus-visible:bg-slate-50 focus-visible:outline-none"
                    >
                      <span className={`flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg ${entry.hasSummary ? 'bg-violet-50 text-violet-600' : entry.hasTranscript ? 'bg-blue-50 text-blue-600' : 'bg-slate-100 text-slate-500'}`}>
                        {entry.hasSummary ? <Sparkles className="h-4 w-4" /> : entry.hasTranscript ? <FileText className="h-4 w-4" /> : <AudioLines className="h-4 w-4" />}
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm font-medium text-slate-900">{entry.title}</span>
                        <span className="mt-0.5 block truncate text-xs text-slate-500 tabular-nums">
                          {[entry.createdAt ? formatRecordedAt(entry.createdAt) : null,
                            entry.durationSeconds !== null ? formatDuration(entry.durationSeconds) : null]
                            .filter(Boolean).join(' · ') || t('common.unknownDate')}
                        </span>
                      </span>
                      <span className="hidden flex-shrink-0 gap-1 sm:flex">
                        {entry.hasTranscript && <span className="chip bg-blue-50 text-blue-700">{t('library.badgeTranscript')}</span>}
                        {entry.hasSummary
                          ? <span className="chip bg-violet-50 text-violet-700">{t('library.badgeSummary')}</span>
                          : entry.hasTranscript && entry.meetingId
                            ? <span className="chip bg-amber-50 text-amber-700">{t('home.recent.needsSummary')}</span>
                            : null}
                      </span>
                      <ChevronRight className="h-4 w-4 flex-shrink-0 text-slate-300 group-hover:text-slate-500" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {/* This week + privacy */}
          <aside className="space-y-4">
            <section className="surface-card p-5">
              <h3 className="text-sm font-semibold text-slate-900">{t('home.week.title')}</h3>
              <div className="mt-4 grid grid-cols-2 gap-4">
                <div>
                  <div className="text-2xl font-semibold tabular-nums text-slate-900">{overview.weekCount}</div>
                  <div className="text-xs text-slate-500">{t('home.week.meetings')}</div>
                </div>
                <div>
                  <div className="text-2xl font-semibold tabular-nums text-slate-900">
                    {week.h > 0 ? `${week.h}` : week.m}
                    <span className="ml-0.5 text-sm font-medium text-slate-500">
                      {week.h > 0 ? t('home.week.hUnit', { m: week.m }) : t('home.week.mUnit')}
                    </span>
                  </div>
                  <div className="text-xs text-slate-500">{t('home.week.duration')}</div>
                </div>
              </div>
              {(overview.needSummary > 0 || overview.needTranscript > 0) && (
                <div className="mt-4 space-y-1.5 border-t border-slate-100 pt-4">
                  {overview.needSummary > 0 && (
                    <button type="button" onClick={() => router.push('/recordings')} className="flex w-full items-center justify-between rounded-lg bg-amber-50 px-3 py-2 text-left text-xs text-amber-800 hover:bg-amber-100">
                      <span>{t('home.week.needSummary', { count: overview.needSummary })}</span>
                      <ChevronRight className="h-3 w-3" />
                    </button>
                  )}
                  {overview.needTranscript > 0 && (
                    <button type="button" onClick={() => router.push('/recordings')} className="flex w-full items-center justify-between rounded-lg bg-slate-50 px-3 py-2 text-left text-xs text-slate-700 hover:bg-slate-100">
                      <span>{t('home.week.needTranscript', { count: overview.needTranscript })}</span>
                      <ChevronRight className="h-3 w-3" />
                    </button>
                  )}
                </div>
              )}
            </section>

            <section className="surface-card p-5">
              <div className="flex items-center justify-between">
                <h3 className="flex items-center gap-1.5 text-sm font-semibold text-slate-900">
                  <ListChecks className="h-4 w-4 text-emerald-600" />{t('home.tasks.title')}
                </h3>
                <button type="button" onClick={() => router.push('/tasks')} className="inline-flex items-center gap-0.5 text-xs font-medium text-blue-600 hover:text-blue-700">
                  {t('home.tasks.viewAll')}<ChevronRight className="h-3 w-3" />
                </button>
              </div>
              <div className="mt-3">
                <ActionItemsPanel compact limit={5} />
              </div>
            </section>

            <button
              type="button"
              onClick={() => router.push('/ask')}
              className="surface-card flex w-full items-center gap-3 p-4 text-left transition-colors hover:border-violet-200 hover:bg-violet-50/40"
            >
              <span className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-violet-50 text-violet-600"><MessageCircleQuestion className="h-4 w-4" /></span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium text-slate-900">{t('home.ask.title')}</span>
                <span className="block truncate text-xs text-slate-500">{t('home.ask.hint')}</span>
              </span>
              <ChevronRight className="h-4 w-4 text-slate-400" />
            </button>

            <section className={`rounded-xl border p-4 ${isLocal ? 'border-emerald-100 bg-emerald-50/60' : 'border-sky-100 bg-sky-50/60'}`}>
              <div className="flex items-start gap-2.5">
                {isLocal
                  ? <ShieldCheck className="mt-0.5 h-4 w-4 flex-shrink-0 text-emerald-600" />
                  : <Cloud className="mt-0.5 h-4 w-4 flex-shrink-0 text-sky-600" />}
                <div>
                  <div className={`text-sm font-medium ${isLocal ? 'text-emerald-900' : 'text-sky-900'}`}>
                    {isLocal ? t('home.privacy.localTitle') : t('home.privacy.cloudTitle')}
                  </div>
                  <p className={`mt-0.5 text-xs leading-relaxed ${isLocal ? 'text-emerald-800/80' : 'text-sky-800/80'}`}>
                    {isLocal ? t('home.privacy.localBody') : t('home.privacy.cloudBody', { provider: PROVIDER_LABELS[provider] ?? provider })}
                  </p>
                </div>
              </div>
            </section>
          </aside>
        </div>
      </div>
    </div>
  );
}
