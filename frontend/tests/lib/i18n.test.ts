import { describe, test, expect, beforeEach } from 'bun:test';
import {
  DEFAULT_LOCALE,
  LOCALES,
  LOCALE_STORAGE_KEY,
  LOCALE_LABELS,
  missingKeys,
  readStoredLocale,
  resolveLocale,
  storeLocale,
  translate,
} from '../../src/lib/i18n';

describe('translate', () => {
  test('returns the key itself when a translation is missing', () => {
    // Returning the key keeps a missing string visible instead of collapsing the
    // label to an empty string.
    expect(translate('zh-CN', 'nope.not.here')).toBe('nope.not.here');
    expect(translate('en', 'nope.not.here')).toBe('nope.not.here');
  });

  test('translates in both locales', () => {
    expect(translate('en', 'settings.title')).toBe('Settings');
    expect(translate('zh-CN', 'settings.title')).toBe('设置');
  });

  test('substitutes named parameters', () => {
    expect(translate('en', 'recording.days', { days: 30 })).toBe('30 days');
    expect(translate('zh-CN', 'recording.days', { days: 30 })).toBe('30 天');
  });

  test('substitutes several parameters in one string', () => {
    const rendered = translate('en', 'dictionary.count', { count: 3, max: 200 });
    expect(rendered).toContain('3');
    expect(rendered).toContain('200');
  });

  test('leaves an unknown placeholder in place rather than writing "undefined"', () => {
    // A template referencing a parameter the caller forgot must be visibly wrong,
    // not silently print "undefined".
    const rendered = translate('en', 'recording.days', {});
    expect(rendered).toBe('{days} days');
    expect(rendered).not.toContain('undefined');
  });

  test('substitutes numeric zero rather than treating it as absent', () => {
    const rendered = translate('en', 'recording.days', { days: 0 });
    expect(rendered).toBe('0 days');
  });

  test('does not interpolate braces in the surrounding text', () => {
    // Chinese summaries contain 「」 rather than braces, but a stray brace must not
    // break rendering.
    expect(translate('en', 'common.cancel')).toBe('Cancel');
  });
});

describe('resolveLocale', () => {
  test('accepts supported values as-is', () => {
    expect(resolveLocale('en')).toBe('en');
    expect(resolveLocale('zh-CN')).toBe('zh-CN');
  });

  test('maps any Chinese tag onto the Chinese table', () => {
    // A stored "zh" or a browser-provided "zh-Hans-CN" should not fall back to
    // English.
    for (const tag of ['zh', 'zh-Hans', 'zh-hans', 'ZH', 'zh-CN', 'zh-SG']) {
      expect(resolveLocale(tag)).toBe('zh-CN');
    }
  });

  test('maps any English tag onto English', () => {
    for (const tag of ['en', 'en-US', 'EN-gb']) {
      expect(resolveLocale(tag)).toBe('en');
    }
  });

  test('falls back to the default for missing or unknown values', () => {
    expect(resolveLocale(null)).toBe(DEFAULT_LOCALE);
    expect(resolveLocale(undefined)).toBe(DEFAULT_LOCALE);
    expect(resolveLocale('')).toBe(DEFAULT_LOCALE);
    expect(resolveLocale('   ')).toBe(DEFAULT_LOCALE);
    expect(resolveLocale('klingon')).toBe(DEFAULT_LOCALE);
  });

  test('the default is a locale the app can actually render', () => {
    expect(LOCALES).toContain(DEFAULT_LOCALE);
  });
});

describe('locale persistence', () => {
  // The hooks are guarded on `window`, which does not exist under the test
  // runner. Install a minimal localStorage so the persistence path is exercised
  // rather than skipped.
  const store = new Map<string, string>();

  beforeEach(() => {
    store.clear();
    (globalThis as unknown as { window: unknown }).window = {
      localStorage: {
        getItem: (key: string) => store.get(key) ?? null,
        setItem: (key: string, value: string) => void store.set(key, value),
        removeItem: (key: string) => void store.delete(key),
        clear: () => store.clear(),
      },
    };
  });

  test('round-trips a choice', () => {
    storeLocale('en');
    expect(readStoredLocale()).toBe('en');
    expect(store.get(LOCALE_STORAGE_KEY)).toBe('en');
  });

  test('defaults when nothing is stored', () => {
    expect(readStoredLocale()).toBe(DEFAULT_LOCALE);
  });

  test('defaults when the stored value is unusable', () => {
    // A hand-edited or stale value must not break startup.
    store.set(LOCALE_STORAGE_KEY, 'not-a-locale');
    expect(readStoredLocale()).toBe(DEFAULT_LOCALE);
  });

  test('accepts a bare language tag from storage', () => {
    store.set(LOCALE_STORAGE_KEY, 'zh');
    expect(readStoredLocale()).toBe('zh-CN');
  });
});

describe('locale catalogues', () => {
  test('every locale has a display label', () => {
    for (const locale of LOCALES) {
      expect(LOCALE_LABELS[locale]?.length ?? 0).toBeGreaterThan(0);
    }
  });

  test('Chinese covers every English key', () => {
    // The Chinese table is the default locale, so a gap here is what most users
    // would see as an English string in a Chinese interface.
    expect(missingKeys('zh-CN')).toEqual([]);
  });

  test('English is complete by definition', () => {
    expect(missingKeys('en')).toEqual([]);
  });
});
