'use client';

import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react';
import {
  DEFAULT_LOCALE,
  type Locale,
  type TranslateParams,
  readStoredLocale,
  storeLocale,
  translate,
} from '@/lib/i18n';

interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  /** Translate `key`, substituting `params`. */
  t: (key: string, params?: TranslateParams) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

/**
 * Provides the interface language.
 *
 * The initial value is read during the first render rather than in an effect, so
 * the first paint is already in the right language — resolving after paint would
 * flash English at a Chinese user on every start.
 *
 * Because the app is statically exported there is no server-side locale to
 * reconcile with, so `localStorage` is the single source of truth.
 */
export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => readStoredLocale());

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    storeLocale(next);
  }, []);

  // Reflect the choice on the document so the browser and any CSS that keys off
  // the language (font selection, line breaking) follow it.
  useEffect(() => {
    if (typeof document === 'undefined') return;
    document.documentElement.lang = locale;
  }, [locale]);

  const value = useMemo<I18nContextValue>(
    () => ({
      locale,
      setLocale,
      t: (key: string, params?: TranslateParams) => translate(locale, key, params),
    }),
    [locale, setLocale],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/**
 * Access the interface language.
 *
 * Falls back to English outside a provider rather than throwing, so a component
 * can be rendered in isolation (a test, a story) without wiring the provider up.
 */
/**
 * Access the interface language.
 *
 * Falls back to the default catalogue outside a provider rather than throwing, so a
 * component can be rendered in isolation (a test, a story) without wiring the provider up.
 *
 * The fallback is a module constant so its identity is stable. Built inline it would be a
 * new object — and a new `t` — on every render, so any component that lists `t` among its
 * `useCallback` or `useEffect` dependencies would re-run that effect forever.
 */
const FALLBACK_I18N: I18nContextValue = {
  locale: DEFAULT_LOCALE,
  setLocale: () => undefined,
  t: (key: string, params?: TranslateParams) => translate(DEFAULT_LOCALE, key, params),
};

export function useI18n(): I18nContextValue {
  return useContext(I18nContext) ?? FALLBACK_I18N;
}
