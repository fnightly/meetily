'use client';

import React, { useState, useMemo, useEffect, useCallback } from 'react';
import { usePrewarmSpeechModel } from '@/hooks/usePrewarmSpeechModel';
import { tr } from '@/lib/i18n';
import { File, Settings, ChevronLeftCircle, ChevronRightCircle, StickyNote, Home, Trash2, Mic, Square, Search, Pencil, NotebookPen, SearchIcon, X, Upload, AudioLines, Loader2, RefreshCw, PanelLeftClose, PanelLeftOpen, MessageCircleQuestion, ListChecks, Briefcase, Hash } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { useRouter, usePathname } from 'next/navigation';
import { useSidebar } from './SidebarProvider';
import type { CurrentMeeting } from '@/components/Sidebar/SidebarProvider';
import { formatBytes, formatDuration, formatRecordedAt } from '@/lib/recordings';
import type { SidebarEntry } from '@/lib/sidebar-entries';
import { ConfirmationModal } from '../ConfirmationModel/confirmation-modal';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { SettingTabs } from '../SettingTabs';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import { invoke } from '@tauri-apps/api/core';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { toast } from 'sonner';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { useConfig } from '@/contexts/ConfigContext';

import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"

import { MessageToast } from '../MessageToast';
import Logo from '../Logo';
import Info from '../Info';
import { ComplianceNotification } from '../ComplianceNotification';
import { Input } from '../ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '../ui/input-group';


function formatClock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

/** Bold every case-insensitive occurrence of the query inside a snippet. */
function markQuery(text: string, query: string) {
  const q = query.trim();
  if (!q) return text;
  const escaped = q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const parts = text.split(new RegExp(`(${escaped})`, 'gi'));
  return parts.map((part, i) => (i % 2 === 1 ? <mark key={i} className="bg-amber-200/80 text-slate-900 rounded-sm">{part}</mark> : part));
}

const Sidebar: React.FC = () => {
  const router = useRouter();
  const { t } = useI18n();
  const pathname = usePathname();

  const {
    currentMeeting,
    setCurrentMeeting,
    isCollapsed,
    toggleCollapse,
    handleRecordingToggle,
    searchTranscripts,
    searchResults,
    isSearching,
    meetings,
    setMeetings,
    serverAddress,
    entries,
    meetingsLoading,
    recordingsLoading,
    recordingsError,
    refetchRecordings
  } = useSidebar();

  // Get recording state from RecordingStateContext (single source of truth)
  const { isRecording } = useRecordingState();
  const { openImportDialog } = useImportDialog();
  const { betaFeatures } = useConfig();
  const [searchQuery, setSearchQuery] = useState<string>('');
  const prewarmHandlers = usePrewarmSpeechModel();
  const [showModelSettings, setShowModelSettings] = useState(false);
  const [modelConfig, setModelConfig] = useState<ModelConfig>({
    provider: 'ollama',
    model: '',
    whisperModel: '',
    apiKey: null,
    ollamaEndpoint: null
  });
  const [transcriptModelConfig, setTranscriptModelConfig] = useState<TranscriptModelProps>({
    provider: 'parakeet',
    model: 'parakeet-tdt-0.6b-v3-int8',
  });
  const [settingsSaveSuccess, setSettingsSaveSuccess] = useState<boolean | null>(null);

  // State for edit modal
  const [editModalState, setEditModalState] = useState<{ isOpen: boolean; meetingId: string | null; currentTitle: string }>({
    isOpen: false,
    meetingId: null,
    currentTitle: ''
  });
  const [editingTitle, setEditingTitle] = useState<string>('');

  // useEffect(() => {
  //   if (settingsSaveSuccess !== null) {
  //     const timer = setTimeout(() => {
  //       setSettingsSaveSuccess(null);
  //     }, 3000);
  //   }
  // }, [settingsSaveSuccess]);


  const [deleteModalState, setDeleteModalState] = useState<{ isOpen: boolean; itemId: string | null }>({ isOpen: false, itemId: null });

  useEffect(() => {
    // Note: Don't set hardcoded defaults - let DB be the source of truth
    const fetchModelConfig = async () => {
      // Only make API call if serverAddress is loaded
      if (!serverAddress) {
        console.log('Waiting for server address to load before fetching model config');
        return;
      }

      try {
        const data = await invoke('api_get_model_config') as any;
        if (data && data.provider !== null) {
          // Fetch API key if not included and provider requires it
          if (data.provider !== 'ollama' && !data.apiKey) {
            try {
              const apiKeyData = await invoke('api_get_api_key', {
                provider: data.provider
              }) as string;
              data.apiKey = apiKeyData;
            } catch (err) {
              console.error('Failed to fetch API key:', err);
            }
          }
          setModelConfig(data);
        }
      } catch (error) {
        console.error('Failed to fetch model config:', error);
      }
    };

    fetchModelConfig();
  }, [serverAddress]);


  useEffect(() => {
    // Note: Don't set hardcoded defaults - let DB be the source of truth
    const fetchTranscriptSettings = async () => {
      // Only make API call if serverAddress is loaded
      if (!serverAddress) {
        console.log('Waiting for server address to load before fetching transcript settings');
        return;
      }

      try {
        const data = await invoke('api_get_transcript_config') as any;
        if (data && data.provider !== null) {
          setTranscriptModelConfig(data);
        }
      } catch (error) {
        console.error('Failed to fetch transcript settings:', error);
      }
    };
    fetchTranscriptSettings();
  }, [serverAddress]);

  // Listen for model config updates from other components
  useEffect(() => {
    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<ModelConfig>('model-config-updated', (event) => {
        console.log('Sidebar received model-config-updated event:', event.payload);
        setModelConfig(event.payload);
      });

      return unlisten;
    };

    let cleanup: (() => void) | undefined;
    setupListener().then(fn => cleanup = fn);

    return () => {
      cleanup?.();
    };
  }, []);



  // Handle model config save
  const handleSaveModelConfig = async (config: ModelConfig) => {
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey,
        ollamaEndpoint: config.ollamaEndpoint,
      });

      setModelConfig(config);
      console.log('Model config saved successfully');
      setSettingsSaveSuccess(true);

      // Emit event to sync other components
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      // Track settings change
    } catch (error) {
      console.error('Error saving model config:', error);
      setSettingsSaveSuccess(false);
    }
  };

  const handleSaveTranscriptConfig = async (updatedConfig?: TranscriptModelProps) => {
    try {
      const configToSave = updatedConfig || transcriptModelConfig;
      const payload = {
        provider: configToSave.provider,
        model: configToSave.model,
        apiKey: configToSave.apiKey ?? null
      };
      console.log('Saving transcript config with payload:', payload);

      await invoke('api_save_transcript_config', {
        provider: payload.provider,
        model: payload.model,
        apiKey: payload.apiKey,
      });


      setSettingsSaveSuccess(true);

      // Track settings change
      const transcriptConfigToSave = updatedConfig || transcriptModelConfig;
    } catch (error) {
      console.error('Failed to save transcript config:', error);
      setSettingsSaveSuccess(false);
    }
  };

  // Search is debounced: every keystroke used to fire a full transcript scan.
  const searchTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const handleSearchChange = useCallback((value: string) => {
    setSearchQuery(value);
    if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    if (!value.trim()) {
      void searchTranscripts('');
      return;
    }
    searchTimerRef.current = setTimeout(() => { void searchTranscripts(value); }, 220);
  }, [searchTranscripts]);

  // Tags / projects of every meeting, for the filter chips and row badges.
  const [labelsByMeeting, setLabelsByMeeting] = useState<Map<string, { tags: string[]; project: string | null }>>(new Map());
  const [labelFilter, setLabelFilter] = useState<{ kind: 'project' | 'tag'; name: string } | null>(null);
  const [openTaskCount, setOpenTaskCount] = useState(0);
  useEffect(() => {
    let alive = true;
    const loadLabels = async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const rows = await invoke<Array<{ meeting_id: string; tags: string[]; project: string | null }>>('api_list_meeting_labels');
        if (alive) setLabelsByMeeting(new Map(rows.map((r) => [r.meeting_id, { tags: r.tags, project: r.project }])));
      } catch { /* labels are optional */ }
    };
    const loadTasks = async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const items = await invoke<Array<{ done: boolean }>>('api_list_action_items', { meetingId: null, includeDone: false });
        if (alive) setOpenTaskCount(items.filter((it) => !it.done).length);
      } catch { /* optional */ }
    };
    void loadLabels();
    void loadTasks();
    window.addEventListener('meetily:labels-changed', loadLabels);
    window.addEventListener('meetily:action-items-changed', loadTasks);
    return () => {
      alive = false;
      window.removeEventListener('meetily:labels-changed', loadLabels);
      window.removeEventListener('meetily:action-items-changed', loadTasks);
    };
  }, []);

  const labelChips = useMemo(() => {
    const projects = new Map<string, number>();
    const tags = new Map<string, number>();
    labelsByMeeting.forEach((l) => {
      if (l.project) projects.set(l.project, (projects.get(l.project) ?? 0) + 1);
      l.tags.forEach((tag) => tags.set(tag, (tags.get(tag) ?? 0) + 1));
    });
    const sorted = (m: Map<string, number>) => [...m.entries()].sort((a, b) => b[1] - a[1]).map(([name]) => name);
    return [
      ...sorted(projects).slice(0, 4).map((name) => ({ kind: 'project' as const, name })),
      ...sorted(tags).slice(0, 6).map((name) => ({ kind: 'tag' as const, name })),
    ];
  }, [labelsByMeeting]);

  // Search hits arrive grouped by meeting (title, labels, summary, notes and
  // transcript), so a row survives the filter when the backend matched it.
  const visibleEntries = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    let list = entries;
    if (labelFilter) {
      list = list.filter((entry) => {
        const l = entry.meetingId ? labelsByMeeting.get(entry.meetingId) : undefined;
        if (!l) return false;
        return labelFilter.kind === 'project' ? l.project === labelFilter.name : l.tags.includes(labelFilter.name);
      });
    }
    if (!query) return list;

    const matchedMeetingIds = new Set(searchResults.map(result => result.id));
    const rank = new Map(searchResults.map((result, i) => [result.id, i]));

    return list
      .filter(entry =>
        entry.title.toLowerCase().includes(query)
        || (entry.meetingId !== null && matchedMeetingIds.has(entry.meetingId))
      )
      .sort((a, b) => (rank.get(a.meetingId ?? '') ?? 999) - (rank.get(b.meetingId ?? '') ?? 999));
  }, [entries, searchQuery, searchResults, labelFilter, labelsByMeeting]);

  // The library is the full archive; the sidebar only needs the most recent rows
  // for quick switching. Showing every meeting here duplicated the library and
  // made the sidebar scroll for no navigation benefit. Searching or filtering
  // still searches the whole set.
  const RECENT_SIDEBAR_LIMIT = 5;
  const showingRecentOnly = !searchQuery.trim() && !labelFilter;
  const listEntries = showingRecentOnly
    ? visibleEntries.slice(0, RECENT_SIDEBAR_LIMIT)
    : visibleEntries;

  const handleDelete = async (itemId: string) => {
    console.log('Deleting item:', itemId);
    const payload = {
      meetingId: itemId
    };

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('api_delete_meeting', {
        meetingId: itemId,
      });
      console.log(tr('toast.meetingDeleted'));
      const updatedMeetings = meetings.filter((m: CurrentMeeting) => m.id !== itemId);
      setMeetings(updatedMeetings);

      // Track meeting deletion

      // Show success toast
      toast.success(tr('toast.meetingDeleted'), {
        description: "All associated data has been removed"
      });

      // If deleting the active meeting, navigate to home
      if (currentMeeting?.id === itemId) {
        setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
        router.push('/');
      }
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      toast.error(tr('toast.deleteMeetingFailed'), {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleDeleteConfirm = () => {
    if (deleteModalState.itemId) {
      handleDelete(deleteModalState.itemId);
    }
    setDeleteModalState({ isOpen: false, itemId: null });
  };

  // Handle modal editing of meeting names
  const handleEditStart = (meetingId: string, currentTitle: string) => {
    setEditModalState({
      isOpen: true,
      meetingId: meetingId,
      currentTitle: currentTitle
    });
    setEditingTitle(currentTitle);
  };

  const handleEditConfirm = async () => {
    const newTitle = editingTitle.trim();
    const meetingId = editModalState.meetingId;

    if (!meetingId) return;

    // Prevent empty titles
    if (!newTitle) {
      toast.error(tr('toast.titleEmpty'));
      return;
    }

    try {
      await invoke('api_save_meeting_title', {
        meetingId: meetingId,
        title: newTitle,
      });

      // Update local state
      const updatedMeetings = meetings.map((m: CurrentMeeting) =>
        m.id === meetingId ? { ...m, title: newTitle } : m
      );
      setMeetings(updatedMeetings);

      // Update current meeting if it's the one being edited
      if (currentMeeting?.id === meetingId) {
        setCurrentMeeting({ id: meetingId, title: newTitle });
      }

      // Track the edit

      toast.success(tr('toast.titleUpdated'));

      // Close modal and reset state
      setEditModalState({ isOpen: false, meetingId: null, currentTitle: '' });
      setEditingTitle('');
    } catch (error) {
      console.error('Failed to update meeting title:', error);
      toast.error(tr('toast.titleUpdateFailed'), {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleEditCancel = () => {
    setEditModalState({ isOpen: false, meetingId: null, currentTitle: '' });
    setEditingTitle('');
  };

  // Expose setShowModelSettings to window for Rust tray to call
  useEffect(() => {
    (window as any).openSettings = () => {
      setShowModelSettings(true);
    };

    // Cleanup on unmount
    return () => {
      delete (window as any).openSettings;
    };
  }, []);

  const isHomePage = pathname === '/';
  const isMeetingPage = pathname?.includes('/meeting-details') ?? false;
  const isRecordingsPage = pathname === '/recordings';
  const isSettingsPage = pathname === '/settings';

  const navItems = [
    { key: 'home', label: t('nav.home'), icon: Home, active: isHomePage, onClick: () => router.push('/') },
    { key: 'ask', label: t('nav.ask'), icon: MessageCircleQuestion, active: pathname === '/ask', onClick: () => router.push('/ask') },
    { key: 'tasks', label: t('nav.tasks'), icon: ListChecks, active: pathname === '/tasks', onClick: () => router.push('/tasks'), badge: openTaskCount },
    { key: 'recordings', label: t('nav.recordings'), icon: AudioLines, active: isRecordingsPage, onClick: () => router.push('/recordings') },
  ];
  const settingsNav = { key: 'settings', label: t('nav.settings'), icon: Settings, active: isSettingsPage, onClick: () => router.push('/settings') };

  const renderCollapsedIcons = () => {
    if (!isCollapsed) return null;

    const railButton = (key: string, label: string, Icon: React.ElementType, onClick: () => void, active = false) => (
      <Tooltip key={key}>
        <TooltipTrigger asChild>
          <button type="button" onClick={onClick} data-active={active} aria-label={label} className="rail-item">
            {active && <span className="absolute -left-3 top-2 bottom-2 w-[3px] rounded-r bg-blue-600" />}
            <Icon className="w-[18px] h-[18px]" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="right"><p>{label}</p></TooltipContent>
      </Tooltip>
    );

    return (
      <TooltipProvider>
        <div className="flex flex-1 flex-col items-center gap-1.5 pt-3">
          <Logo isCollapsed={isCollapsed} />

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                {...prewarmHandlers}
                onClick={handleRecordingToggle}
                disabled={isRecording}
                aria-label={isRecording ? t('nav.recordingInProgress') : t('nav.startRecording')}
                className={`my-2 flex h-10 w-10 items-center justify-center rounded-full bg-record text-white shadow-sm transition-transform hover:scale-105 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-400 focus-visible:ring-offset-2 ${isRecording ? 'animate-rec-pulse cursor-default' : ''}`}
              >
                {isRecording ? <span className="h-3 w-3 rounded-[3px] bg-white" /> : <Mic className="w-[18px] h-[18px]" />}
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>{isRecording ? t('nav.recordingInProgress') : t('nav.startRecording')}</p>
            </TooltipContent>
          </Tooltip>

          {navItems.map((item) => railButton(item.key, item.label, item.icon, item.onClick, item.active))}
          {railButton('meetings', t('nav.library'), NotebookPen, toggleCollapse, isMeetingPage)}
          {betaFeatures.importAndRetranscribe && railButton('import', t('nav.importAudio'), Upload, () => openImportDialog())}

          <div className="mt-auto flex flex-col items-center gap-1.5 pb-3">
            {railButton('settings', t('nav.settings'), Settings, settingsNav.onClick, isSettingsPage)}
            <Info isCollapsed={isCollapsed} />
            {railButton('expand', t('nav.expand'), PanelLeftOpen, toggleCollapse)}
          </div>
        </div>
      </TooltipProvider>
    );
  };

  /**
   * Open the row.
   *
   * A row backed by a meeting opens the meeting; a recording with no meeting row
   * has no details page, so it opens the recordings library, where it can be
   * transcribed, renamed or moved.
   */
  const openEntry = (entry: SidebarEntry) => {
    if (!entry.meetingId) {
      router.push('/recordings');
      return;
    }

    setCurrentMeeting({ id: entry.meetingId, title: entry.title });
    // The meeting id travels as a query parameter rather than a path segment so
    // the static export does not need to enumerate runtime meeting ids.
    router.push(`/meeting-details?id=${encodeURIComponent(entry.meetingId)}`);
  };

  // Transcript search can match a row whose title does not, so the matching
  // excerpt is looked up by meeting id.
  const matchingSnippet = (entry: SidebarEntry) => {
    if (!entry.meetingId || !searchQuery.trim() || !searchResults.length) return null;
    return searchResults.find(result => result.id === entry.meetingId) ?? null;
  };

  const renderEntry = (entry: SidebarEntry) => {
    const isActive = entry.meetingId !== null && currentMeeting?.id === entry.meetingId;
    const snippet = matchingSnippet(entry);

    // Only the facts that are known are shown: a meeting whose recording is gone
    // has no date, duration or size, and three dashes would be noise.
    const details = [
      entry.createdAt ? formatRecordedAt(entry.createdAt) : null,
      entry.durationSeconds !== null ? formatDuration(entry.durationSeconds) : null,
      entry.audioBytes !== null ? formatBytes(entry.audioBytes) : null,
    ].filter((part): part is string => part !== null);

    const actionBtn = 'p-1 rounded-md text-slate-400 focus-visible:outline-none focus-visible:ring-2';

    return (
      <li key={entry.key}>
        <div className={`group relative flex items-start rounded-lg transition-colors duration-150 ${isActive ? 'bg-blue-50' : 'hover:bg-slate-50'}`}>
          <button
            type="button"
            onClick={() => openEntry(entry)}
            aria-label={entry.meetingId
              ? t('nav.openMeeting', { title: entry.title })
              : t('nav.openRecording', { title: entry.title })}
            className="flex-1 min-w-0 flex items-start gap-2.5 px-2 py-2 text-left rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
          >
            <span className={`mt-0.5 flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-md ${isActive ? 'bg-blue-100 text-blue-600' : entry.meetingId ? 'bg-slate-100 text-slate-500' : 'bg-sky-50 text-sky-600'}`}>
              {entry.meetingId
                ? <File className="w-3.5 h-3.5" />
                : <AudioLines className="w-3.5 h-3.5" />}
            </span>

            <span className="flex-1 min-w-0">
              <span className={`block truncate text-[13px] leading-5 ${isActive ? 'font-semibold text-blue-800' : 'font-medium text-slate-800'}`}>
                {entry.title}
              </span>
              <span className="flex items-center gap-1.5 mt-0.5 text-[11px] text-slate-400 min-w-0">
                {details.length > 0 && <span className="truncate tabular-nums">{details.slice(0, 2).join(' · ')}</span>}
                {entry.hasTranscript && (
                  <span className="flex-shrink-0 h-1.5 w-1.5 rounded-full bg-blue-400" title={t('library.badgeTranscript')} />
                )}
                {entry.hasSummary && (
                  <span className="flex-shrink-0 h-1.5 w-1.5 rounded-full bg-violet-400" title={t('library.badgeSummary')} />
                )}
                {entry.meetingId && labelsByMeeting.get(entry.meetingId)?.project && (
                  <span className="truncate rounded bg-blue-50 px-1 text-[10px] text-blue-600">{labelsByMeeting.get(entry.meetingId)?.project}</span>
                )}
              </span>

              {snippet && snippet.hits.length > 0 && (
                <span className="block mt-1 space-y-0.5">
                  {snippet.hits.slice(0, 2).map((hit, i) => (
                    <span
                      key={i}
                      role="link"
                      tabIndex={-1}
                      onClick={(e) => {
                        e.stopPropagation();
                        const q = encodeURIComponent(searchQuery.trim());
                        const time = hit.audio_start_time !== null && hit.audio_start_time !== undefined ? `&t=${Math.floor(hit.audio_start_time)}` : '';
                        router.push(`/meeting-details?id=${encodeURIComponent(entry.meetingId as string)}${time}&q=${q}`);
                      }}
                      className="block text-[11px] text-slate-600 bg-amber-50 hover:bg-amber-100 px-1.5 py-1 rounded border border-amber-100 line-clamp-2 cursor-pointer"
                    >
                      <span className="font-medium text-amber-600">
                        {t(`search.source.${hit.source}`)}
                        {hit.audio_start_time !== null && hit.audio_start_time !== undefined ? ` ${formatClock(hit.audio_start_time)}` : ''}
                      </span>{' '}
                      {markQuery(hit.snippet, searchQuery)}
                    </span>
                  ))}
                  {snippet.totalHits > 2 && (
                    <span className="block px-1.5 text-[10px] text-slate-400">{t('search.moreHits', { count: snippet.totalHits - 2 })}</span>
                  )}
                </span>
              )}
            </span>
          </button>

          {entry.meetingId && (
            // Hidden until the row is hovered or anything inside it has keyboard focus, so the
            // list stays calm without locking keyboard users out of these actions.
            <div className={`absolute right-1 top-1.5 flex items-center gap-0.5 rounded-md pl-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 ${isActive ? 'bg-blue-50' : 'bg-slate-50'}`}>
              <button
                type="button"
                onClick={() => router.push(`/notes?id=${encodeURIComponent(entry.meetingId as string)}`)}
                className={`${actionBtn} hover:text-amber-600 hover:bg-amber-50 focus-visible:ring-amber-500`}
                aria-label={t('nav.openNotes', { title: entry.title })}
                title={t('nav.notes')}
              >
                <StickyNote className="w-3.5 h-3.5" />
              </button>
              <button
                type="button"
                onClick={() => handleEditStart(entry.meetingId as string, entry.title)}
                className={`${actionBtn} hover:text-blue-600 hover:bg-blue-50 focus-visible:ring-blue-500`}
                aria-label={t('nav.renameMeeting', { title: entry.title })}
                title={t('common.rename')}
              >
                <Pencil className="w-3.5 h-3.5" />
              </button>
              <button
                type="button"
                onClick={() => setDeleteModalState({ isOpen: true, itemId: entry.meetingId })}
                className={`${actionBtn} hover:text-red-600 hover:bg-red-50 focus-visible:ring-red-500`}
                aria-label={t('nav.deleteMeeting', { title: entry.title })}
                title={t('common.delete')}
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
            </div>
          )}
        </div>
      </li>
    );
  };

  return (
    <div className="fixed top-0 left-0 h-screen z-40">
      <div
        className={`h-screen bg-white border-r border-slate-200 flex flex-col transition-[width] duration-300 ${isCollapsed ? 'w-16' : 'w-64'}`}
      >
        {isCollapsed ? renderCollapsedIcons() : (
          <>
            {/* Brand + collapse */}
            <div className="flex-shrink-0 flex items-center justify-between gap-2 px-3 pt-4 pb-3">
              <div className="flex-1 min-w-0"><Logo isCollapsed={false} /></div>
              <button
                type="button"
                onClick={toggleCollapse}
                aria-label={t('nav.collapse')}
                title={t('nav.collapse')}
                className="rounded-lg p-1.5 text-slate-400 hover:bg-slate-100 hover:text-slate-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
              >
                <PanelLeftClose className="w-4 h-4" />
              </button>
            </div>

            {/* Primary actions */}
            <div className="flex-shrink-0 px-3 flex gap-2">
              <button
                type="button"
                {...prewarmHandlers}
                onClick={handleRecordingToggle}
                disabled={isRecording}
                className={`flex-1 flex items-center justify-center gap-2 h-10 rounded-xl text-sm font-semibold text-white shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-400 focus-visible:ring-offset-2 ${isRecording ? 'bg-red-400 cursor-default' : 'bg-record hover:bg-red-600'}`}
              >
                {isRecording ? (
                  <>
                    <span className="relative flex h-2.5 w-2.5">
                      <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-white opacity-75" />
                      <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-white" />
                    </span>
                    <span>{t('nav.recordingInProgress')}</span>
                  </>
                ) : (
                  <>
                    <Mic className="w-4 h-4" />
                    <span>{t('nav.startRecording')}</span>
                  </>
                )}
              </button>
              {betaFeatures.importAndRetranscribe && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      onClick={() => openImportDialog()}
                      aria-label={t('nav.importAudio')}
                      className="h-10 w-10 flex items-center justify-center rounded-xl border border-slate-200 text-slate-600 hover:bg-slate-50 hover:text-slate-900 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                    >
                      <Upload className="w-4 h-4" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="bottom"><p>{t('nav.importAudio')}</p></TooltipContent>
                </Tooltip>
              )}
            </div>

            {/* Navigation */}
            <nav className="flex-shrink-0 px-3 pt-4 space-y-0.5" aria-label={t('nav.primary')}>
              {navItems.map(({ key, label, icon: Icon, active, onClick, badge }) => (
                <button key={key} type="button" onClick={onClick} data-active={active} className="nav-item">
                  <Icon className="w-4 h-4" />
                  <span className="flex-1 text-left">{label}</span>
                  {badge ? <span className="rounded-full bg-slate-100 px-1.5 text-[10px] font-semibold tabular-nums text-slate-500">{badge}</span> : null}
                </button>
              ))}
            </nav>

            {/* Meetings list */}
            <div className="flex-1 flex flex-col min-h-0 pt-4">
              <div className="flex-shrink-0 px-3">
                <div className="flex items-center justify-between px-1 pb-2">
                  <span className="eyebrow">
                    {showingRecentOnly ? t('nav.recent') : t('nav.library')}
                  </span>
                  {searchQuery && isSearching ? (
                    <span className="text-[11px] text-blue-500 animate-pulse">{t('nav.searching')}</span>
                  ) : (
                    <span className="text-[11px] text-slate-400 tabular-nums">{visibleEntries.length}</span>
                  )}
                </div>
                <InputGroup className="h-8 bg-slate-50 border-slate-200">
                  <InputGroupInput
                    placeholder={t('nav.searchPlaceholder')}
                    value={searchQuery}
                    onChange={(e) => handleSearchChange(e.target.value)}
                    className="text-[13px]"
                  />
                  <InputGroupAddon>
                    <SearchIcon />
                  </InputGroupAddon>
                  {searchQuery && (
                    <InputGroupAddon align={'inline-end'}>
                      <InputGroupButton onClick={() => handleSearchChange('')} aria-label={t('common.clear')}>
                        <X />
                      </InputGroupButton>
                    </InputGroupAddon>
                  )}
                </InputGroup>
                {labelChips.length > 0 && (
                  <div className="mt-2 flex gap-1 overflow-x-auto pb-0.5 no-scrollbar" role="group" aria-label={t('labels.filter')}>
                    {labelChips.map((chip) => {
                      const active = labelFilter?.kind === chip.kind && labelFilter.name === chip.name;
                      return (
                        <button
                          key={`${chip.kind}:${chip.name}`}
                          type="button"
                          aria-pressed={active}
                          onClick={() => setLabelFilter(active ? null : chip)}
                          className={`flex-shrink-0 inline-flex items-center gap-0.5 rounded-full px-2 py-0.5 text-[11px] transition-colors ${active ? 'bg-slate-900 text-white' : 'bg-slate-100 text-slate-600 hover:bg-slate-200'}`}
                        >
                          {chip.kind === 'project' ? <Briefcase className="h-2.5 w-2.5" /> : <Hash className="h-2.5 w-2.5" />}
                          <span className="max-w-[90px] truncate">{chip.name}</span>
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>

              <div className="flex-1 overflow-y-auto custom-scrollbar min-h-0 pt-2 pb-2">
                {recordingsError && (
                  <div
                    role="alert"
                    className="mx-3 mb-1 p-2 rounded-lg bg-red-50 border border-red-200 text-xs text-red-800 flex items-center gap-2"
                  >
                    <span className="flex-1">{t('library.loadFailed')}</span>
                    <button
                      type="button"
                      onClick={() => void refetchRecordings()}
                      className="inline-flex items-center gap-1 px-2 py-1 rounded border border-red-300 bg-white hover:bg-red-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
                    >
                      <RefreshCw className="w-3 h-3" />
                      {t('common.retry')}
                    </button>
                  </div>
                )}

                {recordingsLoading && (
                  <div className="mx-4 mb-1 flex items-center gap-2 text-xs text-slate-500">
                    <Loader2 className="w-3 h-3 animate-spin" />
                    {t('library.loading')}
                  </div>
                )}

                {!meetingsLoading && !recordingsLoading && !recordingsError && visibleEntries.length === 0 && (
                  <div className="mx-3 mt-1 p-3 rounded-lg border border-dashed border-slate-200 text-xs text-slate-500 leading-relaxed">
                    {searchQuery.trim()
                      ? t('nav.noMatches', { query: searchQuery.trim() })
                      : t('nav.emptyLibrary')}
                  </div>
                )}

                {listEntries.length > 0 && (
                  <ul className="mx-2 space-y-px">
                    {listEntries.map(renderEntry)}
                  </ul>
                )}

                {showingRecentOnly && visibleEntries.length > RECENT_SIDEBAR_LIMIT && (
                  <button
                    type="button"
                    onClick={() => router.push('/recordings')}
                    className="mx-3 mt-2 flex w-[calc(100%-1.5rem)] items-center justify-center gap-1 rounded-lg border border-slate-200 px-2 py-1.5 text-[11px] font-medium text-slate-500 transition-colors hover:border-slate-300 hover:bg-slate-50 hover:text-slate-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                  >
                    {t('nav.viewAllRecordings', { count: visibleEntries.length })}
                    <ChevronRightCircle className="h-3 w-3" />
                  </button>
                )}
              </div>
            </div>

            {/* Footer */}
            <div className="flex-shrink-0 border-t border-slate-100 px-3 py-2 flex items-center gap-1">
              <button
                type="button"
                onClick={settingsNav.onClick}
                data-active={isSettingsPage}
                className="nav-item flex-1"
              >
                <Settings className="w-4 h-4" />
                <span>{t('nav.settings')}</span>
              </button>
              <Info isCollapsed={true} />
              <span className="px-1 text-[11px] text-slate-300 tabular-nums">v0.4.1</span>
            </div>
          </>
        )}
      </div>

      {/* Confirmation Modal for Delete */}
      <ConfirmationModal
        isOpen={deleteModalState.isOpen}
        text={t('nav.deleteConfirm')}
        onConfirm={handleDeleteConfirm}
        onCancel={() => setDeleteModalState({ isOpen: false, itemId: null })}
      />

      {/* Edit Meeting Title Modal */}
      <Dialog open={editModalState.isOpen} onOpenChange={(open) => {
        if (!open) handleEditCancel();
      }}>
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>{t('nav.editTitle')}</DialogTitle>
          </VisuallyHidden>
          <div className="py-2">
            <h3 className="text-base font-semibold text-slate-900 mb-4">{t('nav.editTitle')}</h3>
            <label htmlFor="meeting-title" className="block text-sm font-medium text-slate-700 mb-2">
              {t('nav.titleLabel')}
            </label>
            <input
              id="meeting-title"
              type="text"
              value={editingTitle}
              onChange={(e) => setEditingTitle(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  handleEditConfirm();
                } else if (e.key === 'Escape') {
                  handleEditCancel();
                }
              }}
              className="w-full h-10 px-3 border border-slate-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              placeholder={t('nav.titlePlaceholder')}
              autoFocus
            />
          </div>
          <DialogFooter>
            <button
              onClick={handleEditCancel}
              className="px-4 h-9 text-sm font-medium text-slate-700 bg-slate-100 hover:bg-slate-200 rounded-lg transition-colors"
            >
              {t('common.cancel')}
            </button>
            <button
              onClick={handleEditConfirm}
              className="px-4 h-9 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-lg transition-colors"
            >
              {t('common.save')}
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default Sidebar;
