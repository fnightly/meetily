'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { BookA, Loader2, Plus, RefreshCw, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useI18n } from '@/contexts/I18nContext';

interface DictionaryEntry {
  term: string;
  misheard_as?: string | null;
}

interface Dictionary {
  entries: DictionaryEntry[];
}

/** Mirrors `MAX_ENTRIES` on the Rust side. */
const MAX_ENTRIES = 200;

/**
 * The user's custom vocabulary.
 *
 * Proper nouns — colleagues' names, product names, internal jargon — are what a
 * speech model reliably gets wrong, and no general model can know them. Telling
 * the model the exact spelling is far more reliable than correcting the output
 * afterwards. For Chinese it matters more than usual: names differing by one
 * character are different people, and a homophone guess reads as plausible.
 */
export function DictionarySettings() {
  const { t } = useI18n();
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [newTerm, setNewTerm] = useState('');
  const [newMisheard, setNewMisheard] = useState('');

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const dictionary = await invoke<Dictionary>('get_dictionary');
      setEntries(dictionary.entries ?? []);
    } catch (error) {
      console.error('Failed to load dictionary:', error);
      toast.error(t('dictionary.loadFailed'));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /** Persist and adopt the sanitised result the backend returns. */
  const persist = async (next: DictionaryEntry[]) => {
    setSaving(true);
    try {
      const saved = await invoke<Dictionary>('set_dictionary', {
        dictionary: { entries: next },
      });
      // The backend trims, dedupes and caps; showing its result keeps the UI
      // honest about what is actually stored.
      setEntries(saved.entries ?? []);
    } catch (error) {
      console.error('Failed to save dictionary:', error);
      toast.error(t('dictionary.saveFailed'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  const handleAdd = async () => {
    const term = newTerm.trim();
    if (!term) return;

    if (entries.length >= MAX_ENTRIES) {
      toast.error(t('dictionary.limitReached', { max: MAX_ENTRIES }));
      return;
    }

    const misheard = newMisheard.trim();
    await persist([
      ...entries,
      { term, misheard_as: misheard && misheard !== term ? misheard : null },
    ]);

    setNewTerm('');
    setNewMisheard('');
  };

  const handleRemove = async (term: string) => {
    await persist(entries.filter((entry) => entry.term !== term));
  };

  const handleTermChange = (index: number, value: string) => {
    setEntries((current) =>
      current.map((entry, i) => (i === index ? { ...entry, term: value } : entry)),
    );
  };

  const handleMisheardChange = (index: number, value: string) => {
    setEntries((current) =>
      current.map((entry, i) => (i === index ? { ...entry, misheard_as: value } : entry)),
    );
  };

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h4 className="text-base font-medium text-gray-900">{t('dictionary.title')}</h4>
          <p className="text-sm text-gray-600 mt-1">
            {t('dictionary.description')}
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
          {loading ? (
            <Loader2 className="w-4 h-4 animate-spin" />
          ) : (
            <RefreshCw className="w-4 h-4" />
          )}
          <span className="ml-2">{t('common.reload')}</span>
        </Button>
      </div>

      {/* Adding */}
      <div className="surface-card p-5 space-y-3">
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="space-y-1">
            <label htmlFor="dict-term" className="text-xs font-medium text-gray-700">
              {t('dictionary.correctSpelling')}
            </label>
            <Input
              id="dict-term"
              value={newTerm}
              onChange={(event) => setNewTerm(event.target.value)}
              placeholder={t('dictionary.termPlaceholder')}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void handleAdd();
              }}
            />
          </div>
          <div className="space-y-1">
            <label htmlFor="dict-misheard" className="text-xs font-medium text-gray-700">
              Often misheard as <span className="text-gray-400">{t('dictionary.optional')}</span>
            </label>
            <Input
              id="dict-misheard"
              value={newMisheard}
              onChange={(event) => setNewMisheard(event.target.value)}
              placeholder={t('dictionary.misheardPlaceholder')}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void handleAdd();
              }}
            />
          </div>
        </div>
        <div className="flex items-center justify-between gap-3">
          <p className="text-xs text-gray-500">
            {t('dictionary.addHint')}
          </p>
          <Button onClick={() => void handleAdd()} disabled={!newTerm.trim() || saving}>
            <Plus className="w-4 h-4" />
            <span className="ml-2">{t('common.add')}</span>
          </Button>
        </div>
      </div>

      {/* Listing */}
      {loading && entries.length === 0 ? (
        <div className="flex items-center gap-2 text-sm text-gray-500 p-4">
          <Loader2 className="w-4 h-4 animate-spin" />
          {t('common.loading')}
        </div>
      ) : entries.length === 0 ? (
        <div className="p-6 border rounded-lg text-center text-sm text-gray-500">
          <BookA className="w-6 h-6 mx-auto mb-2 text-gray-300" />
          {t('dictionary.empty')}
        </div>
      ) : (
        <div className="space-y-2">
          {entries.map((entry, index) => (
            <div key={`${entry.term}-${index}`} className="flex items-center gap-2">
              <Input
                value={entry.term}
                onChange={(event) => handleTermChange(index, event.target.value)}
                onBlur={() => void persist(entries)}
                className="flex-1"
              />
              <span className="text-gray-400 text-sm shrink-0">←</span>
              <Input
                value={entry.misheard_as ?? ''}
                onChange={(event) => handleMisheardChange(index, event.target.value)}
                onBlur={() => void persist(entries)}
                placeholder="misheard as (optional)"
                className="flex-1"
              />
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void handleRemove(entry.term)}
                disabled={saving}
                className="text-red-600 hover:text-red-700 shrink-0"
                title={t('dictionary.removeTitle')}
              >
                <Trash2 className="w-4 h-4" />
              </Button>
            </div>
          ))}
        </div>
      )}

      <p className="text-xs text-gray-500">
        {t('dictionary.count', { count: entries.length, max: MAX_ENTRIES })}
      </p>
    </div>
  );
}
