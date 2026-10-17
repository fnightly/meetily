'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AppWindow, Check, Info, Loader2, MonitorSpeaker, RefreshCw, Search } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useI18n } from '@/contexts/I18nContext';
import {
  filterWindows,
  isSameWindow,
  primaryWindowLabel,
  secondaryWindowLabel,
  type RecordableWindow,
} from '@/lib/window-picker';

export interface WindowPickerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The chosen window, or null when the whole system mix is recorded. */
  selected: RecordableWindow | null;
  /** Called with the chosen window, or null for whole-system audio. */
  onSelect: (window: RecordableWindow | null) => void;
}

/**
 * Chooses which application a recording follows.
 *
 * Windows are listed with the title the user recognises and the executable name
 * behind it, plus a marker for the ones already producing sound. The first row
 * is always the whole-system mix, so the feature is additive: the default
 * behaviour stays one click away.
 */
export function WindowPickerDialog({
  open,
  onOpenChange,
  selected,
  onSelect,
}: WindowPickerDialogProps) {
  const { t } = useI18n();
  const [windows, setWindows] = useState<RecordableWindow[]>([]);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const [query, setQuery] = useState('');

  const loadWindows = useCallback(async () => {
    setLoading(true);
    setFailed(false);
    try {
      const result = await invoke<RecordableWindow[]>('list_recordable_windows');
      setWindows(result ?? []);
    } catch (error) {
      console.error('Failed to list recordable windows:', error);
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  // Refetch whenever the dialog opens: windows (and whether they are playing)
  // change while the app runs, so a list cached from last time would be stale.
  useEffect(() => {
    if (!open) return;
    setQuery('');
    void loadWindows();
  }, [open, loadWindows]);

  const visibleWindows = useMemo(() => filterWindows(windows, query), [windows, query]);

  const choose = (window: RecordableWindow | null) => {
    onSelect(window);
    onOpenChange(false);
  };

  const rowClass = (active: boolean) =>
    `group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60 ${
      active ? 'bg-blue-50 ring-1 ring-inset ring-blue-200' : 'hover:bg-slate-50'
    }`;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] max-w-xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{t('windowPicker.title')}</DialogTitle>
          <DialogDescription>{t('windowPicker.description')}</DialogDescription>
        </DialogHeader>

        <div className="flex items-center gap-2">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
            <Input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t('windowPicker.search')}
              aria-label={t('windowPicker.search')}
              autoFocus
              className="h-9 border-slate-200 pl-8 text-sm"
            />
          </div>
          <button
            type="button"
            onClick={() => void loadWindows()}
            disabled={loading}
            aria-label={t('windowPicker.refresh')}
            title={t('windowPicker.refresh')}
            className="inline-flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-md border border-slate-200 text-slate-500 transition-colors hover:bg-slate-50 hover:text-slate-900 disabled:pointer-events-none disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
          >
            <RefreshCw className={`h-4 w-4 ${loading ? 'animate-spin' : ''}`} />
          </button>
        </div>

        {failed ? (
          <div className="flex h-[260px] flex-col items-center justify-center gap-3 rounded-lg border border-dashed border-slate-200 px-6 text-center">
            <p className="text-sm text-slate-600">{t('windowPicker.error')}</p>
            <button
              type="button"
              onClick={() => void loadWindows()}
              className="inline-flex items-center gap-1.5 rounded-md bg-blue-600 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-blue-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t('windowPicker.retry')}
            </button>
          </div>
        ) : loading && windows.length === 0 ? (
          <div className="flex h-[260px] items-center justify-center gap-2 text-sm text-slate-500">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t('windowPicker.loading')}
          </div>
        ) : (
          <div className="min-h-0 space-y-1">
            <button
              type="button"
              onClick={() => choose(null)}
              aria-pressed={selected === null}
              className={rowClass(selected === null)}
            >
              <span
                className={`flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-lg ${
                  selected === null ? 'bg-blue-100 text-blue-700' : 'bg-slate-100 text-slate-500'
                }`}
              >
                <MonitorSpeaker className="h-4 w-4" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium text-slate-900">
                  {t('windowPicker.wholeSystem')}
                </span>
                <span className="mt-0.5 block truncate text-xs text-slate-500">
                  {t('windowPicker.wholeSystemHint')}
                </span>
              </span>
              {selected === null && <Check className="h-4 w-4 flex-shrink-0 text-blue-600" />}
            </button>

            {windows.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-1 px-6 py-10 text-center">
                <AppWindow className="h-6 w-6 text-slate-300" />
                <p className="text-sm text-slate-600">{t('windowPicker.empty')}</p>
                <p className="text-xs text-slate-400">{t('windowPicker.emptyHint')}</p>
              </div>
            ) : visibleWindows.length === 0 ? (
              <p className="px-3 py-10 text-center text-sm text-slate-500">
                {t('windowPicker.noMatch')}
              </p>
            ) : (
              <ScrollArea className="h-[260px]">
                <ul className="space-y-1 pr-2">
                  {visibleWindows.map((window) => {
                    const active = isSameWindow(selected, window);
                    const secondary = secondaryWindowLabel(window);
                    return (
                      <li key={`${window.handle}-${window.process_id}`}>
                        <button
                          type="button"
                          onClick={() => choose(window)}
                          aria-pressed={active}
                          title={primaryWindowLabel(window)}
                          className={rowClass(active)}
                        >
                          <span
                            className={`flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-lg ${
                              active ? 'bg-blue-100 text-blue-700' : 'bg-slate-100 text-slate-500'
                            }`}
                          >
                            <AppWindow className="h-4 w-4" />
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-sm font-medium text-slate-900">
                              {primaryWindowLabel(window)}
                            </span>
                            {(secondary || window.is_playing_audio) && (
                              <span className="mt-0.5 flex min-w-0 items-center gap-1.5">
                                {secondary && (
                                  <span className="truncate text-xs text-slate-500">{secondary}</span>
                                )}
                                {window.is_playing_audio && (
                                  <span className="chip flex-shrink-0 bg-emerald-50 text-emerald-700">
                                    {t('windowPicker.playing')}
                                  </span>
                                )}
                              </span>
                            )}
                          </span>
                          {active && <Check className="h-4 w-4 flex-shrink-0 text-blue-600" />}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </ScrollArea>
            )}
          </div>
        )}

        <p className="flex items-start gap-2 rounded-lg bg-slate-50 px-3 py-2 text-xs leading-relaxed text-slate-500">
          <Info className="mt-0.5 h-3.5 w-3.5 flex-shrink-0 text-slate-400" />
          {t('windowPicker.note')}
        </p>
      </DialogContent>
    </Dialog>
  );
}
