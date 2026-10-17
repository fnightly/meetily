'use client';

import { useCallback, useEffect, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle2,
  Database,
  FolderOpen,
  KeyRound,
  Loader2,
  Save,
  Share2,
  TestTube2,
  Trash2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { useI18n } from '@/contexts/I18nContext';
import {
  formatSyncTags,
  getSyncSettings,
  parseSyncTags,
  pickObsidianFolder,
  saveSyncSettings,
  testNotionConnection,
  type SyncSettings,
  type SyncSettingsView,
} from '@/lib/sync';

type Busy = 'save' | 'pick' | 'test' | 'remove' | null;

/**
 * Obsidian / Notion configuration.
 *
 * The sync itself lives on the meeting page: this panel only says where notes
 * would go. The Notion token is write-only from the renderer's point of view —
 * the backend reports whether one is stored, never the value — so an empty field
 * always means "keep what is saved".
 */
export function SyncSettingsPanel() {
  const { t } = useI18n();

  const [view, setView] = useState<SyncSettingsView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [folder, setFolder] = useState('');
  const [subfolder, setSubfolder] = useState('');
  const [template, setTemplate] = useState('');
  const [tags, setTags] = useState('');
  const [databaseId, setDatabaseId] = useState('');
  const [token, setToken] = useState('');

  const applyView = useCallback((next: SyncSettingsView) => {
    setView(next);
    setFolder(next.settings.obsidian_folder ?? '');
    setSubfolder(next.settings.obsidian_subfolder ?? '');
    setTemplate(next.settings.obsidian_template ?? '');
    setTags(formatSyncTags(next.settings.obsidian_tags));
    setDatabaseId(next.settings.notion_database_id ?? '');
    setToken('');
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      applyView(await getSyncSettings());
    } catch (cause) {
      setError(`${t('sync.settings.loadFailed')}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setLoading(false);
    }
  }, [applyView, t]);

  useEffect(() => {
    void load();
  }, [load]);

  const currentSettings = (): SyncSettings => ({
    obsidian_folder: folder.trim() || null,
    obsidian_subfolder: subfolder.trim() || null,
    obsidian_template: template.trim() || null,
    obsidian_tags: parseSyncTags(tags),
    notion_database_id: databaseId.trim() || null,
  });

  const handleSave = async () => {
    setBusy('save');
    setError(null);
    setNotice(null);
    try {
      const next = await saveSyncSettings(currentSettings(), token.trim() ? token : undefined);
      applyView(next);
      setNotice(t('sync.settings.saved'));
    } catch (cause) {
      setError(`${t('sync.settings.saveFailed')}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setBusy(null);
    }
  };

  const handleRemoveToken = async () => {
    setBusy('remove');
    setError(null);
    setNotice(null);
    try {
      // An explicit empty string is the backend's "delete the stored token".
      const next = await saveSyncSettings(currentSettings(), '');
      applyView(next);
      setNotice(t('sync.settings.tokenRemoved'));
    } catch (cause) {
      setError(`${t('sync.settings.saveFailed')}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setBusy(null);
    }
  };

  const handlePickFolder = async () => {
    setBusy('pick');
    setError(null);
    setNotice(null);
    try {
      const picked = await pickObsidianFolder();
      if (picked) setFolder(picked);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  };

  const handleTest = async () => {
    setBusy('test');
    setError(null);
    setNotice(null);
    try {
      const result = await testNotionConnection();
      const database =
        result.database_title || result.data_source_name || t('sync.target.notion');
      setNotice(t('sync.settings.testOk', { database, property: result.title_property }));
    } catch (cause) {
      setError(`${t('sync.settings.testFailed')}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setBusy(null);
    }
  };

  if (loading && !view) {
    return (
      <div className="surface-card p-6">
        <h3 className="text-base font-semibold text-slate-900 mb-2">{t('sync.settings.title')}</h3>
        <p className="flex items-center gap-2 text-sm text-slate-500">
          <Loader2 className="w-4 h-4 animate-spin" />
          {t('common.loading')}
        </p>
      </div>
    );
  }

  const customFolder = folder.trim().length > 0;
  const displayFolder = customFolder ? folder.trim() : view?.effective_obsidian_folder ?? '';
  const tokenStored = view?.notion_token_set ?? false;

  return (
    <div className="space-y-6">
      <div className="surface-card p-6">
        <div className="mb-5">
          <h3 className="flex items-center gap-2 text-base font-semibold text-slate-900">
            <Share2 className="w-4 h-4 text-slate-500" />
            {t('sync.settings.title')}
          </h3>
          <p className="mt-1 text-sm text-slate-500">{t('sync.settings.description')}</p>
        </div>

        {error && (
          <div role="alert" className="mb-4 flex gap-2 rounded-md border border-red-200 bg-red-50 p-3">
            <AlertTriangle className="mt-0.5 w-4 h-4 shrink-0 text-red-600" />
            <p className="text-xs text-red-800">{error}</p>
          </div>
        )}

        {notice && !error && (
          <div role="status" className="mb-4 flex gap-2 rounded-md border border-green-200 bg-green-50 p-3">
            <CheckCircle2 className="mt-0.5 w-4 h-4 shrink-0 text-green-600" />
            <p className="text-xs text-green-900">{notice}</p>
          </div>
        )}

        <div className="space-y-6">
          {/* Obsidian */}
          <section className="rounded-lg border border-slate-200 bg-slate-50/70 p-4">
            <h4 className="font-medium text-slate-900">{t('sync.settings.obsidianTitle')}</h4>
            <p className="mt-1 text-xs text-slate-500">{t('sync.settings.obsidianDesc')}</p>

            <div className="mt-4 space-y-4">
              <div>
                <div className="mb-1 flex items-center gap-2">
                  <label htmlFor="obsidian-folder" className="text-sm font-medium text-slate-900">
                    {t('sync.settings.folder')}
                  </label>
                  <span
                    className={`rounded px-1.5 py-0.5 text-[10px] uppercase tracking-wide ${
                      customFolder ? 'bg-blue-100 text-blue-800' : 'bg-gray-200 text-gray-700'
                    }`}
                  >
                    {customFolder ? t('sync.settings.custom') : t('sync.settings.default')}
                  </span>
                </div>
                <div className="mb-2 break-all rounded-md border border-slate-200 bg-white px-3 py-2 font-mono text-xs text-slate-700">
                  {displayFolder || '—'}
                </div>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void handlePickFolder()}
                    disabled={busy !== null}
                  >
                    {busy === 'pick' ? (
                      <Loader2 className="w-4 h-4 animate-spin" />
                    ) : (
                      <FolderOpen className="w-4 h-4" />
                    )}
                    <span className="ml-2">{t('sync.settings.changeFolder')}</span>
                  </Button>
                  {customFolder && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setFolder('')}
                      disabled={busy !== null}
                    >
                      {t('sync.settings.useDefault')}
                    </Button>
                  )}
                </div>
                <p className="mt-2 text-xs text-slate-500">
                  {t('sync.settings.notesWillBe', { path: view?.obsidian_notes_folder ?? displayFolder })}
                </p>
              </div>

              <div>
                <label htmlFor="obsidian-subfolder" className="mb-1 block text-sm font-medium text-slate-900">
                  {t('sync.settings.subfolder')}
                </label>
                <Input
                  id="obsidian-subfolder"
                  value={subfolder}
                  onChange={(event) => setSubfolder(event.target.value)}
                  placeholder="Meetings/2026"
                  spellCheck={false}
                />
                <p className="mt-1 text-xs text-slate-500">{t('sync.settings.subfolderHint')}</p>
              </div>

              <div>
                <label htmlFor="obsidian-tags" className="mb-1 block text-sm font-medium text-slate-900">
                  {t('sync.settings.tags')}
                </label>
                <Input
                  id="obsidian-tags"
                  value={tags}
                  onChange={(event) => setTags(event.target.value)}
                  placeholder={t('sync.tagsPlaceholder')}
                  spellCheck={false}
                />
                <p className="mt-1 text-xs text-slate-500">{t('sync.settings.tagsHint')}</p>
              </div>

              <div>
                <label htmlFor="obsidian-template" className="mb-1 block text-sm font-medium text-slate-900">
                  {t('sync.settings.template')}
                </label>
                <Textarea
                  id="obsidian-template"
                  value={template}
                  onChange={(event) => setTemplate(event.target.value)}
                  rows={3}
                  spellCheck={false}
                  placeholder={'## {{title}}\n\n{{content}}'}
                />
                <p className="mt-1 text-xs text-slate-500">{t('sync.settings.templateHint')}</p>
              </div>
            </div>
          </section>

          {/* Notion */}
          <section className="rounded-lg border border-slate-200 bg-slate-50/70 p-4">
            <h4 className="flex items-center gap-2 font-medium text-slate-900">
              <Database className="w-4 h-4 text-slate-500" />
              {t('sync.settings.notionTitle')}
            </h4>
            <p className="mt-1 text-xs text-slate-500">{t('sync.settings.notionDesc')}</p>

            <div className="mt-4 space-y-4">
              <div>
                <div className="mb-1 flex items-center gap-2">
                  <label htmlFor="notion-database" className="text-sm font-medium text-slate-900">
                    {t('sync.settings.databaseId')}
                  </label>
                  {databaseId.trim() !== '' && (
                    <CheckCircle2 className="w-3.5 h-3.5 text-green-600" />
                  )}
                </div>
                <Input
                  id="notion-database"
                  value={databaseId}
                  onChange={(event) => setDatabaseId(event.target.value)}
                  placeholder={t('sync.settings.databasePlaceholder')}
                  spellCheck={false}
                  autoComplete="off"
                />
                <p className="mt-1 text-xs text-slate-500">{t('sync.settings.databaseIdHint')}</p>
              </div>

              <div>
                <div className="mb-1 flex items-center gap-2">
                  <label htmlFor="notion-token" className="text-sm font-medium text-slate-900">
                    {t('sync.settings.token')}
                  </label>
                  <KeyRound className="w-3.5 h-3.5 text-slate-400" />
                  {tokenStored && (
                    <span className="rounded bg-green-100 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-green-800">
                      {t('sync.settings.tokenSaved')}
                    </span>
                  )}
                </div>
                <Input
                  id="notion-token"
                  type="password"
                  value={token}
                  onChange={(event) => setToken(event.target.value)}
                  placeholder={
                    tokenStored ? t('sync.settings.tokenStored') : t('sync.settings.tokenPlaceholder')
                  }
                  spellCheck={false}
                  autoComplete="off"
                />
                <p className="mt-1 text-xs text-slate-500">{t('sync.settings.tokenHint')}</p>
                {tokenStored && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="mt-2 text-red-600 hover:text-red-700"
                    onClick={() => void handleRemoveToken()}
                    disabled={busy !== null}
                  >
                    <Trash2 className="w-4 h-4" />
                    <span className="ml-2">{t('sync.settings.removeToken')}</span>
                  </Button>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleTest()}
                  disabled={busy !== null || !tokenStored || databaseId.trim() === ''}
                  title={
                    !tokenStored || databaseId.trim() === ''
                      ? t('sync.settings.testNeedsSave')
                      : undefined
                  }
                >
                  {busy === 'test' ? (
                    <Loader2 className="w-4 h-4 animate-spin" />
                  ) : (
                    <TestTube2 className="w-4 h-4" />
                  )}
                  <span className="ml-2">
                    {busy === 'test' ? t('sync.settings.testing') : t('sync.settings.test')}
                  </span>
                </Button>
                {(!tokenStored || databaseId.trim() === '') && (
                  <span className="text-xs text-slate-500">{t('sync.settings.testNeedsSave')}</span>
                )}
              </div>
            </div>
          </section>
        </div>

        <div className="mt-5 flex items-center justify-between gap-4">
          <p className="max-w-xl text-xs text-slate-500">{t('sync.settings.privacy')}</p>
          <Button onClick={() => void handleSave()} disabled={busy !== null} className="shrink-0">
            {busy === 'save' ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Save className="w-4 h-4" />
            )}
            <span className="ml-2">
              {busy === 'save' ? t('sync.settings.saving') : t('sync.settings.save')}
            </span>
          </Button>
        </div>
      </div>
    </div>
  );
}
