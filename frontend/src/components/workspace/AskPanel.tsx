'use client';

import { Fragment, useEffect, useRef, useState, type ReactNode } from 'react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { formatSpeakerLabel } from '@/lib/speaker-labels';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { ArrowUp, Loader2, MessageCircleQuestion, Quote, Sparkles } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';

export interface AskSource {
  id: string;
  kind: 'transcript' | 'summary' | string;
  meeting_id: string;
  meeting_title: string;
  created_at: string;
  audio_start_time: number | null;
  speaker: string | null;
  snippet: string;
  cited: boolean;
}

interface AskAnswer {
  answer: string;
  sources: AskSource[];
  model: string;
}

interface Turn {
  question: string;
  answer?: AskAnswer;
  error?: string;
}

function clock(seconds: number | null): string {
  if (seconds === null || seconds === undefined || seconds < 0) return '';
  const s = Math.floor(seconds);
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

export function sourceHref(source: Pick<AskSource, 'meeting_id' | 'audio_start_time'>): string {
  const base = `/meeting-details?id=${encodeURIComponent(source.meeting_id)}`;
  return source.audio_start_time !== null && source.audio_start_time !== undefined
    ? `${base}&t=${Math.max(0, Math.floor(source.audio_start_time))}`
    : base;
}

/** Turn "[S2]" markers in text into clickable citation chips. */
function withCitations(children: ReactNode, onCite: (id: string) => void): ReactNode {
  const walk = (node: ReactNode): ReactNode => {
    if (typeof node === 'string') {
      const parts = node.split(/(\[S\d{1,3}\](?:\s*\[S\d{1,3}\])*)/g);
      return parts.map((part, i) => {
        const ids = part.match(/S\d{1,3}/g);
        if (!ids || !/^\[S/.test(part)) return <Fragment key={i}>{part}</Fragment>;
        return (
          <Fragment key={i}>
            {ids.map((id) => (
              <button key={id} type="button" onClick={() => onCite(id)}
                className="mx-0.5 inline-flex h-4 items-center rounded bg-blue-100 px-1 align-[1px] text-[10px] font-semibold text-blue-700 hover:bg-blue-200">
                {id.slice(1)}
              </button>
            ))}
          </Fragment>
        );
      });
    }
    if (Array.isArray(node)) return node.map((n, i) => <Fragment key={i}>{walk(n)}</Fragment>);
    return node;
  };
  return walk(children);
}

function SourceList({ sources, highlight }: { sources: AskSource[]; highlight: string | null }) {
  const { t, locale } = useI18n();
  const router = useRouter();
  const cited = sources.filter((s) => s.cited);
  const shown = cited.length > 0 ? cited : sources.slice(0, 4);
  if (shown.length === 0) return null;
  return (
    <div className="mt-3 space-y-1.5">
      <div className="eyebrow">{t('ask.sources')}</div>
      {shown.map((s) => {
        const date = new Date(s.created_at);
        const when = Number.isNaN(date.getTime()) ? '' : date.toLocaleDateString(locale, { month: 'short', day: 'numeric' });
        return (
          <button
            key={s.id}
            id={`ask-source-${s.id}`}
            type="button"
            onClick={() => router.push(sourceHref(s))}
            className={`block w-full rounded-lg border px-3 py-2 text-left transition-colors hover:border-blue-300 hover:bg-blue-50/40 ${highlight === s.id ? 'border-blue-400 bg-blue-50' : 'border-slate-200 bg-white'}`}
          >
            <div className="flex items-center gap-1.5 text-[11px] text-slate-500">
              <span className="rounded bg-blue-100 px-1 font-semibold text-blue-700">{s.id.slice(1)}</span>
              <span className="truncate font-medium text-slate-700">{s.meeting_title || t('live.untitled')}</span>
              {when && <span>· {when}</span>}
              {s.kind === 'summary'
                ? <span className="chip bg-violet-50 text-violet-600"><Sparkles className="h-3 w-3" />{t('ask.summarySource')}</span>
                : <span className="tabular-nums">{clock(s.audio_start_time)}{s.speaker ? ` · ${formatSpeakerLabel(s.speaker, t)}` : ''}</span>}
            </div>
            <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-slate-600">{s.snippet}</p>
          </button>
        );
      })}
    </div>
  );
}

/** Ask questions about one meeting (`meetingId`) or about the whole library. */
export function AskPanel({ meetingId = null, className = '' }: { meetingId?: string | null; className?: string }) {
  const { t } = useI18n();
  const router = useRouter();
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => { endRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' }); }, [turns]);

  const ask = async (question: string) => {
    const q = question.trim();
    if (!q || busy) return;
    setDraft('');
    setBusy(true);
    setTurns((prev) => [...prev, { question: q }]);
    try {
      const answer = await invoke<AskAnswer>('api_ask_meetings', { question: q, meetingId });
      setTurns((prev) => prev.map((turn, i) => (i === prev.length - 1 ? { ...turn, answer } : turn)));
    } catch (e) {
      setTurns((prev) => prev.map((turn, i) => (i === prev.length - 1 ? { ...turn, error: String(e) } : turn)));
    } finally {
      setBusy(false);
    }
  };

  const suggestions = meetingId
    ? [t('ask.suggest.decisions'), t('ask.suggest.todos'), t('ask.suggest.numbers'), t('ask.suggest.risks')]
    : [t('ask.suggest.globalTodo'), t('ask.suggest.globalPrice'), t('ask.suggest.globalRisk')];

  const onCite = (id: string) => {
    setHighlight(id);
    document.getElementById(`ask-source-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  };

  return (
    <div className={`flex h-full min-h-0 flex-col ${className}`}>
      <div className="min-h-0 flex-1 overflow-y-auto pr-1">
        {turns.length === 0 ? (
          <div className="flex flex-col items-center px-4 py-10 text-center">
            <span className="mb-3 flex h-11 w-11 items-center justify-center rounded-full bg-violet-50 text-violet-600"><MessageCircleQuestion className="h-5 w-5" /></span>
            <p className="text-sm font-medium text-slate-700">{meetingId ? t('ask.emptyMeeting') : t('ask.emptyGlobal')}</p>
            <p className="mt-1 max-w-sm text-xs text-slate-400">{t('ask.emptyHint')}</p>
            <div className="mt-4 flex flex-wrap justify-center gap-1.5">
              {suggestions.map((s) => (
                <button key={s} type="button" onClick={() => void ask(s)} className="chip border border-slate-200 bg-white text-slate-600 hover:border-violet-300 hover:text-violet-700">{s}</button>
              ))}
            </div>
          </div>
        ) : (
          <div className="space-y-5 py-2">
            {turns.map((turn, i) => (
              <div key={i} className="space-y-2">
                <div className="flex justify-end">
                  <div className="max-w-[85%] rounded-2xl rounded-br-md bg-blue-600 px-3.5 py-2 text-sm text-white">{turn.question}</div>
                </div>
                {turn.answer ? (
                  <div className="surface-card p-4">
                    <div className="prose prose-sm max-w-none text-slate-800 prose-p:my-1.5 prose-li:my-0.5 prose-headings:mb-1.5 prose-headings:mt-3">
                      <ReactMarkdown
                        remarkPlugins={[remarkGfm]}
                        components={{
                          p: ({ children }) => <p>{withCitations(children, onCite)}</p>,
                          li: ({ children }) => <li>{withCitations(children, onCite)}</li>,
                        }}
                      >
                        {turn.answer.answer}
                      </ReactMarkdown>
                    </div>
                    <SourceList sources={turn.answer.sources} highlight={highlight} />
                    {turn.answer.model && <div className="mt-2 text-right text-[10px] text-slate-400">{t('ask.answeredBy', { model: turn.answer.model })}</div>}
                  </div>
                ) : turn.error ? (
                  <div className="rounded-xl border border-red-200 bg-red-50 px-3.5 py-2.5 text-sm text-red-700">
                    {turn.error}
                    {/(模型|API Key|配置)/.test(turn.error) && (
                      <button type="button" onClick={() => router.push('/settings')} className="ml-2 font-medium underline">{t('ask.openSettings')}</button>
                    )}
                  </div>
                ) : (
                  <div className="flex items-center gap-2 px-1 text-sm text-slate-500"><Loader2 className="h-4 w-4 animate-spin text-violet-500" />{t('ask.thinking')}</div>
                )}
              </div>
            ))}
            <div ref={endRef} />
          </div>
        )}
      </div>
      <div className="pt-3">
        <div className="flex items-end gap-2 rounded-xl border border-slate-200 bg-white p-2 shadow-sm focus-within:border-violet-300 focus-within:ring-2 focus-within:ring-violet-500/20">
          <Quote className="mb-2 ml-1 h-4 w-4 flex-shrink-0 text-slate-300" />
          <textarea
            value={draft}
            rows={1}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void ask(draft);
              }
            }}
            placeholder={meetingId ? t('ask.placeholderMeeting') : t('ask.placeholderGlobal')}
            aria-label={t('ask.title')}
            className="max-h-32 min-h-[36px] flex-1 resize-none bg-transparent py-2 text-sm focus:outline-none"
          />
          <button
            type="button"
            disabled={busy || !draft.trim()}
            onClick={() => void ask(draft)}
            aria-label={t('ask.send')}
            className="flex h-9 w-9 items-center justify-center rounded-lg bg-violet-600 text-white transition-colors hover:bg-violet-700 disabled:bg-slate-200 disabled:text-slate-400"
          >
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <ArrowUp className="h-4 w-4" />}
          </button>
        </div>
        <p className="mt-1.5 px-1 text-[11px] text-slate-400">{t('ask.privacy')}</p>
      </div>
    </div>
  );
}
