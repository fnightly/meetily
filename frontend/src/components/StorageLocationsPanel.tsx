"use client"

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
  label: string
  description: string
  /** Which resolved field to display. */
  resolvedKey: keyof ResolvedStoragePaths
  /** Which config field determines whether the value is customised. */
  configKey: keyof StorageConfig
}

const ROWS: RowSpec[] = [
  {
    kind: 'database',
    label: 'Database',
    description: 'Meeting, transcript and summary records (SQLite).',
    resolvedKey: 'database_file',
    configKey: 'database_folder',
  },
  {
    kind: 'models',
    label: 'Models',
    description: 'Downloaded Whisper / Parakeet / built-in summary models.',
    resolvedKey: 'models_folder',
    configKey: 'models_folder',
  },
  {
    kind: 'notes',
    label: 'Meeting Notes',
    description: 'Where user-written meeting notes are kept.',
    resolvedKey: 'notes_folder',
    configKey: 'notes_folder',
  },
  {
    kind: 'recordings',
    label: 'Meeting Recordings',
    description: 'Recorded audio files.',
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
        setNotice('Reset to the default location.')
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
      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <h3 className="text-lg font-semibold text-gray-900 mb-4">Data Storage Locations</h3>
        <p className="text-sm text-gray-600">Loading storage locations…</p>
      </div>
    )
  }

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
      <div className="flex items-start justify-between mb-4">
        <div>
          <h3 className="text-lg font-semibold text-gray-900">Data Storage Locations</h3>
          <p className="text-sm text-gray-600 mt-1">
            Choose where Meetily keeps each kind of data. Leave a location at its default to keep
            everything under the application data directory.
          </p>
        </div>
        <button
          onClick={refresh}
          title="Reload from disk"
          aria-label="Reload storage locations"
          className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors shrink-0"
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
            The database location changed but the running app still has the old file open. Your data
            has been copied to the new location — <strong>restart Meetily</strong> to start using it.
            <br />
            <span className="font-mono break-all">In use now: {view.active_database_file}</span>
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
            <div key={row.kind} className="p-4 border rounded-lg bg-gray-50">
              <div className="flex items-center gap-2 mb-1">
                <span className="font-medium">{row.label}</span>
                {isCustom ? (
                  <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-blue-100 text-blue-800">
                    Custom
                  </span>
                ) : (
                  <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-gray-200 text-gray-700">
                    Default
                  </span>
                )}
              </div>
              <p className="text-xs text-gray-500 mb-2">{row.description}</p>
              <div className="text-xs text-gray-700 mb-3 break-all font-mono">{current || '—'}</div>

              {/* The configured models folder can be empty while models sit in
                  the default location, because changing the setting does not
                  move files. Say which folder is actually in use, or the folder
                  above looks like it is being ignored. */}
              {row.kind === 'models' && view?.models_folder_fallback && (
                <div className="mb-3 p-2 bg-amber-50 border border-amber-200 rounded flex gap-2">
                  <AlertTriangle className="w-4 h-4 text-amber-600 mt-0.5 shrink-0" />
                  <p className="text-xs text-amber-900">
                    This folder has no models, so Meetily is loading them from{' '}
                    <span className="font-mono break-all">{view.effective_models_folder}</span>.
                    Move your models into the folder above (or re-download them) to use it.
                  </p>
                </div>
              )}

              <div className="flex flex-wrap gap-2">
                <button
                  onClick={() => handleChange(row.kind)}
                  disabled={busy}
                  className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors disabled:opacity-50"
                >
                  <FolderOpen className="w-4 h-4" />
                  {busy ? 'Working…' : 'Change…'}
                </button>
                <button
                  onClick={() => handleOpen(row.kind)}
                  className="px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors"
                >
                  Open Folder
                </button>
                {isCustom && (
                  <button
                    onClick={() => handleReset(row.kind)}
                    disabled={busy}
                    className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors disabled:opacity-50"
                  >
                    <RotateCcw className="w-4 h-4" />
                    Reset
                  </button>
                )}
              </div>
            </div>
          )
        })}
      </div>

      <div className="mt-4 p-3 bg-blue-50 rounded-md">
        <p className="text-xs text-blue-800">
          <strong>Note:</strong> Changing the models location does not move models that were already
          downloaded — move the files yourself or re-download them. Changing the database location
          copies your data and requires a restart.
        </p>
      </div>
    </div>
  )
}
