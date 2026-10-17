"use client"
import { tr } from '@/lib/i18n'

import { useCallback, useEffect, useState } from "react"
import { invoke } from "@tauri-apps/api/core"
import { FolderOpen, RotateCcw, RefreshCw, AlertTriangle, Check } from "lucide-react"

/**
 * Editable storage-location settings.
 *
 * The backend (`src/storage/`) owns path resolution and persistence. This panel
 * only chooses and displays; it deliberately does not construct paths itself so
 * that "default" handling stays in one place.
 *
 * Database relocation cannot take effect in the running process (the pool is
 * already open), so the backend snapshots the file and reports
 * `restart_required`; we surface that as an explicit notice.
 */

export interface StorageConfig {
  database_folder: string | null
  models_folder: string | null
  notes_folder: string | null
  recordings_folder: string | null
}

export interface ResolvedStoragePaths {
  database_folder: string
  database_file: string
  models_folder: string
  notes_folder: string
  recordings_folder: string
  app_data_dir: string
}

export interface StorageSettingsView {
  config: StorageConfig
  resolved: ResolvedStoragePaths
  active_database_file: string
  restart_required: boolean
  /** Where models actually load from (may differ from the configured folder). */
  effective_models_folder: string
  /** True when the configured models folder is empty and the default is in use. */
  models_folder_fallback: boolean
}

export interface DatabaseMigrationOutcome {
  destination: string
  copied: boolean
  restart_required: boolean
  detail: string
}

type StorageKind = 'database' | 'models' | 'notes' | 'recordings'

interface RowSpec {
  kind: StorageKind
  labelKey: string
  descKey: string
  /** Which resolved field to display. */
  resolvedKey: keyof ResolvedStoragePaths
  /** Which config field determines whether the value is customised. */
  configKey: keyof StorageConfig
}

const ROWS: RowSpec[] = [
  {
    kind: 'database',
    labelKey: 'storage.db',
    descKey: 'storage.dbDesc',
    resolvedKey: 'database_file',
    configKey: 'database_folder',
  },
  {
    kind: 'models',
    labelKey: 'storage.models',
    descKey: 'storage.modelsDesc',
    resolvedKey: 'models_folder',
    configKey: 'models_folder',
  },
  {
    kind: 'notes',
    labelKey: 'storage.notes',
    descKey: 'storage.notesDesc',
    resolvedKey: 'notes_folder',
    configKey: 'notes_folder',
  },
  {
    kind: 'recordings',
    labelKey: 'storage.recordings',
    descKey: 'storage.recordingsDesc',
    resolvedKey: 'recordings_folder',
    configKey: 'recordings_folder',
  },
]

export function StorageLocationsPanel() {
  const [view, setView] = useState<StorageSettingsView | null>(null)
  const [loading, setLoading] = useState(true)
  const [busyKind, setBusyKind] = useState<StorageKind | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<StorageSettingsView>('get_storage_config')
      setView(next)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  const persist = useCallback(
    async (next: StorageConfig) => {
      try {
        const outcome = await invoke<DatabaseMigrationOutcome>('set_storage_config', {
          config: next,
        })
        setNotice(outcome.detail)
        setError(null)
        await refresh()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
        // Re-read so the UI cannot show a change that was rejected.
        await refresh()
      }
    },
    [refresh],
  )

  const handleChange = useCallback(
    async (kind: StorageKind) => {
      setBusyKind(kind)
      setError(null)
      setNotice(null)
      try {
        const picked = await invoke<string | null>('pick_storage_directory', { kind })
        if (!picked || !view) return

        const configKey = ROWS.find((r) => r.kind === kind)!.configKey
        await persist({ ...view.config, [configKey]: picked })
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusyKind(null)
      }
    },
    [view, persist],
  )

  const handleReset = useCallback(
    async (kind: StorageKind) => {
      setBusyKind(kind)
      setError(null)
      setNotice(null)
      try {
        const next = await invoke<StorageSettingsView>('reset_storage_location', { kind })
        setView(next)
        setNotice(tr('storage.resetDone'))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusyKind(null)
      }
    },
    [],
  )

  const handleOpen = useCallback(async (kind: StorageKind) => {
    try {
      await invoke('open_storage_folder', { kind })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  if (loading) {
    return (
      <div className="surface-card p-6">
        <h3 className="text-base font-semibold text-slate-900 mb-4">{tr('storage.title')}</h3>
        <p className="text-sm text-slate-500">{tr('common.loading')}</p>
      </div>
    )
  }

  return (
    <div className="surface-card p-6">
      <div className="flex items-start justify-between mb-4">
        <div>
          <h3 className="text-base font-semibold text-slate-900">{tr('storage.title')}</h3>
          <p className="text-sm text-slate-500 mt-1">{tr('storage.description')}</p>
        </div>
        <button
          onClick={refresh}
          title={tr('common.reload')}
          aria-label={tr('common.reload')}
          className="flex items-center gap-2 px-3 h-8 text-sm border border-slate-300 bg-white rounded-lg hover:bg-slate-50 transition-colors shrink-0"
        >
          <RefreshCw className="w-4 h-4" />
        </button>
      </div>

      {view?.restart_required && (
        <div
          role="status"
          className="mb-4 p-3 bg-amber-50 border border-amber-200 rounded-md flex gap-2"
        >
          <AlertTriangle className="w-4 h-4 text-amber-600 mt-0.5 shrink-0" />
          <p className="text-xs text-amber-900">
            {tr('storage.restartNeeded')}
            <br />
            <span className="font-mono break-all">{tr('storage.inUse')} {view.active_database_file}</span>
          </p>
        </div>
      )}

      {error && (
        <div role="alert" className="mb-4 p-3 bg-red-50 border border-red-200 rounded-md">
          <p className="text-xs text-red-800">{error}</p>
        </div>
      )}

      {notice && !error && (
        <div role="status" className="mb-4 p-3 bg-green-50 border border-green-200 rounded-md flex gap-2">
          <Check className="w-4 h-4 text-green-600 mt-0.5 shrink-0" />
          <p className="text-xs text-green-900">{notice}</p>
        </div>
      )}

      <div className="space-y-4">
        {ROWS.map((row) => {
          const current = view?.resolved[row.resolvedKey] ?? ''
          const isCustom = Boolean(view?.config[row.configKey])
          const busy = busyKind === row.kind

          return (
            <div key={row.kind} className="p-4 border border-slate-200 rounded-lg bg-slate-50/70">
              <div className="flex items-center gap-2 mb-1">
                <span className="font-medium text-slate-900">{tr(row.labelKey)}</span>
                {isCustom ? (
                  <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-blue-100 text-blue-800">
                    {tr('storage.custom')}
                  </span>
                ) : (
                  <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-gray-200 text-gray-700">
                    {tr('storage.default')}
                  </span>
                )}
              </div>
              <p className="text-xs text-slate-500 mb-2">{tr(row.descKey)}</p>
              <div className="text-xs text-gray-700 mb-3 break-all font-mono">{current || '—'}</div>

              {/* The configured models folder can be empty while models sit in
                  the default location, because changing the setting does not
                  move files. Say which folder is actually in use, or the folder
                  above looks like it is being ignored. */}
              {row.kind === 'models' && view?.models_folder_fallback && (
                <div className="mb-3 p-2 bg-amber-50 border border-amber-200 rounded flex gap-2">
                  <AlertTriangle className="w-4 h-4 text-amber-600 mt-0.5 shrink-0" />
                  <p className="text-xs text-amber-900">
                    {tr('storage.modelsFallback')}{' '}
                    <span className="font-mono break-all">{view.effective_models_folder}</span>
                  </p>
                </div>
              )}

              <div className="flex flex-wrap gap-2">
                <button
                  onClick={() => handleChange(row.kind)}
                  disabled={busy}
                  className="flex items-center gap-2 px-3 h-8 text-sm border border-slate-300 bg-white rounded-lg hover:bg-slate-50 transition-colors disabled:opacity-50"
                >
                  <FolderOpen className="w-4 h-4" />
                  {busy ? tr('storage.working') : tr('storage.change')}
                </button>
                <button
                  onClick={() => handleOpen(row.kind)}
                  className="px-3 h-8 text-sm border border-slate-300 bg-white rounded-lg hover:bg-slate-50 transition-colors"
                >
                  {tr('storage.open')}
                </button>
                {isCustom && (
                  <button
                    onClick={() => handleReset(row.kind)}
                    disabled={busy}
                    className="flex items-center gap-2 px-3 h-8 text-sm border border-slate-300 bg-white rounded-lg hover:bg-slate-50 transition-colors disabled:opacity-50"
                  >
                    <RotateCcw className="w-4 h-4" />
                    {tr('storage.reset')}
                  </button>
                )}
              </div>
            </div>
          )
        })}
      </div>

      <div className="mt-4 p-3 bg-blue-50 rounded-md">
        <p className="text-xs text-blue-800">
          {tr('storage.note')}
        </p>
      </div>
    </div>
  )
}
