/**
 * Background transcription of saved audio.
 *
 * Transcribing a recording (from the library, the meeting page or the
 * post-recording prompt) and importing an audio file used to hold the app in a
 * modal progress dialog until the backend finished, so nothing else could be
 * done meanwhile. Both now run here instead: a caller queues the work and moves
 * on, a single toast shows the running job's progress, and when the queue
 * drains the user gets a toast with a link to the result plus a system
 * notification if the window is in the background.
 *
 * Module-level rather than React state, like lib/background-finalize: a job must
 * survive navigating away from the page that started it, and every entry point
 * shares one queue per backend command (each command allows one job at a time).
 * The ordering rules for recordings live in the pure lib/transcription-queue.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { tr } from '@/lib/i18n';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import { batchTranscriptionTarget, type ActiveSpeechSettings } from '@/lib/transcription-target';
import { batchTargetForModel, type ModelOption } from '@/lib/transcription-model-options';
import {
  initialTranscriptionQueue,
  nextQueuedFolder,
  transcriptionQueueReducer,
  transcriptionStageKey,
  type TranscriptionQueueEvent,
  type TranscriptionQueueItem,
  type TranscriptionQueueState,
  type TranscriptionStatus,
} from '@/lib/transcription-queue';

/** The backend's single-job guards reject a second start with this wording. */
const BUSY_ERROR = 'already in progress';
const BUSY_POLL_MS = 1_000;
const BUSY_WAIT_TIMEOUT_MS = 30 * 60 * 1_000;

/** Key the meeting page reads to generate a summary the user asked for. */
const REQUESTED_SUMMARY_KEY = 'meetily.requestedSummaryMeeting';

type ModelChoice = Pick<ModelOption, 'provider' | 'name'>;

export interface TranscribeRequest {
  folderPath: string;
  /** The meeting transcripts attach to; created from the folder when missing. */
  meetingId?: string | null;
  /** Shown in progress and completion messages; the folder name when omitted. */
  title?: string;
  /** Model for this recording only; a retry without one keeps the earlier choice. */
  model?: ModelChoice;
  /** A language code, or null for detection; omitted uses the Settings language. */
  language?: string | null;
  /** Ask the meeting page for a summary once the transcript exists. */
  summarize?: boolean;
}

export interface ImportRequest {
  sourcePath: string;
  title: string;
  language?: string | null;
  model?: string | null;
  provider?: string | null;
}

export interface ImportJob {
  id: string;
  title: string;
  status: TranscriptionStatus;
  stage: string;
  percentage: number;
  meetingId?: string;
  segmentsCount?: number;
  errorMessage?: string;
}

export interface BackgroundTranscriptionSnapshot {
  recordings: TranscriptionQueueState;
  imports: ImportJob[];
}

/** A job that reached a final state, for pages that reload when one does. */
export interface SettledJob {
  kind: 'transcribe' | 'import';
  status: 'done' | 'error' | 'cancelled';
  title: string;
  folderPath?: string;
  meetingId?: string | null;
  segmentsCount?: number;
  errorMessage?: string;
}

/** App wiring the module cannot reach on its own; set once by the provider. */
export interface BackgroundTranscriptionHost {
  navigate: (href: string) => void;
  /** Reload the meeting list after a job adds or changes a meeting. */
  refreshMeetings?: () => void | Promise<void>;
}

// --- Pure helpers ----------------------------------------------------------

/** Last path component, used as a title when the caller has none. */
export function folderTitle(folderPath: string): string {
  const parts = folderPath.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? folderPath;
}

/**
 * The command arguments for one recording: the Settings engine, the model the
 * user picked for it if any, and the language they picked if any. Parakeet has
 * no language option, so it never receives one.
 */
export function transcribeArgs(
  settings: ActiveSpeechSettings | null,
  model: ModelChoice | undefined,
  language: string | null | undefined,
) {
  const base = batchTranscriptionTarget(settings);
  const target = model ? batchTargetForModel(base, model) : base;
  const chosenLanguage = language === undefined ? target.language : language;
  return { ...target, language: target.provider === 'parakeet' ? null : chosenLanguage };
}

/** What one lane finished since it last started from idle. */
export interface LaneRun {
  done: Array<{ title: string; meetingId?: string | null; segmentsCount?: number }>;
  failed: Array<{ title: string; message: string }>;
}

export interface RunAnnouncement {
  kind: 'success' | 'partial' | 'error';
  title: string;
  description?: string;
  /** Where "View" leads: the meeting for one result, else the library. */
  href: string;
}

/**
 * The message for a lane that just went idle, or null when nothing finished
 * (everything was cancelled). One result links to its meeting; several link to
 * the library, where each row shows its own outcome.
 */
export function describeRun(kind: SettledJob['kind'], run: LaneRun): RunAnnouncement | null {
  const done = run.done.length;
  const failed = run.failed.length;
  if (done + failed === 0) return null;

  if (done === 1 && failed === 0) {
    const [only] = run.done;
    return {
      kind: 'success',
      title: tr(kind === 'import' ? 'bgTranscribe.importDone' : 'bgTranscribe.doneOne', { name: only.title }),
      description: only.segmentsCount !== undefined
        ? tr('bgTranscribe.segments', { count: only.segmentsCount })
        : undefined,
      href: only.meetingId ? `/meeting-details?id=${only.meetingId}` : '/recordings',
    };
  }

  if (done === 0 && failed === 1) {
    const [only] = run.failed;
    return {
      kind: 'error',
      title: tr(kind === 'import' ? 'bgTranscribe.importFailed' : 'bgTranscribe.failedOne', { name: only.title }),
      description: only.message,
      href: '/recordings',
    };
  }

  return {
    kind: failed === 0 ? 'success' : done === 0 ? 'error' : 'partial',
    title: tr('bgTranscribe.doneMany', { count: done + failed }),
    description: failed > 0 ? tr('bgTranscribe.someFailed', { done, failed }) : undefined,
    href: '/recordings',
  };
}

// --- State -----------------------------------------------------------------

interface RecordingOptions {
  title: string;
  model?: ModelChoice;
  language?: string | null;
  summarize?: boolean;
}

let recordings: TranscriptionQueueState = initialTranscriptionQueue;
let imports: ImportJob[] = [];
let snapshot: BackgroundTranscriptionSnapshot = { recordings, imports };

const recordingOptions = new Map<string, RecordingOptions>();
const importRequests = new Map<string, ImportRequest>();
/** meeting id -> folder, set before the start so an early event still matches. */
const meetingToFolder = new Map<string, string>();
const cancelledFolders = new Set<string>();
/** Guards against starting the same item twice while its start is in flight. */
const startingFolders = new Set<string>();
let activeImport: string | null = null;
let cancelledImport: string | null = null;
let importSeq = 0;

const changeListeners = new Set<() => void>();
const settledListeners = new Set<(job: SettledJob) => void>();
let host: BackgroundTranscriptionHost | null = null;

const runs: Record<SettledJob['kind'], LaneRun> = {
  transcribe: { done: [], failed: [] },
  import: { done: [], failed: [] },
};
/** The progress toast was closed by the user; stay quiet until the lane drains. */
const progressHidden: Record<SettledJob['kind'], boolean> = { transcribe: false, import: false };
const lastProgressText: Record<SettledJob['kind'], string> = { transcribe: '', import: '' };
/** Bumped per run so a late dismissal of the previous run's toast is ignored. */
const runGeneration: Record<SettledJob['kind'], number> = { transcribe: 0, import: 0 };
const TOAST_ID: Record<SettledJob['kind'], string> = {
  transcribe: 'background-transcription',
  import: 'background-import',
};

function publish() {
  snapshot = { recordings, imports };
  changeListeners.forEach((listener) => listener());
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const isPendingStatus = (status: TranscriptionStatus) => status === 'queued' || status === 'running';

// --- Public store API ------------------------------------------------------

export function subscribeBackgroundTranscription(listener: () => void) {
  changeListeners.add(listener);
  return () => {
    changeListeners.delete(listener);
  };
}

export function getBackgroundTranscriptionSnapshot(): BackgroundTranscriptionSnapshot {
  return snapshot;
}

/** Called with every job that finishes, fails or is cancelled. */
export function onBackgroundJobSettled(listener: (job: SettledJob) => void) {
  settledListeners.add(listener);
  return () => {
    settledListeners.delete(listener);
  };
}

export function configureBackgroundTranscription(next: BackgroundTranscriptionHost | null) {
  host = next;
}

/** Queue recordings; one already queued or running is left as it is. */
export function enqueueTranscription(requests: TranscribeRequest[]) {
  void ensureBackgroundTranscriptionListeners();
  for (const request of requests) {
    const existing = recordings.items[request.folderPath];
    if (existing && isPendingStatus(existing.status)) continue;

    const previous = recordingOptions.get(request.folderPath);
    recordingOptions.set(request.folderPath, {
      title: request.title || previous?.title || folderTitle(request.folderPath),
      model: request.model ?? previous?.model,
      language: request.language !== undefined ? request.language : previous?.language,
      summarize: request.summarize ?? false,
    });
    if (request.meetingId) meetingToFolder.set(request.meetingId, request.folderPath);
    dispatchRecording({ type: 'enqueue', folderPath: request.folderPath, meetingId: request.meetingId ?? null });
  }
}

/** Cancel one queued or running recording. */
export function cancelTranscription(folderPath: string) {
  const item = recordings.items[folderPath];
  if (!item || !isPendingStatus(item.status)) return;

  if (item.status === 'queued' && recordings.active !== folderPath) {
    // Still in line: nothing to stop at the backend.
    settleRecording(folderPath, { type: 'cancelled', folderPath });
    return;
  }

  cancelledFolders.add(folderPath);
  if (item.status === 'running') {
    void invoke('cancel_retranscription_command').catch((error) => {
      // The run may have settled meanwhile; its event moves the queue on.
      console.error('[BackgroundTranscription] Failed to cancel:', error);
    });
  }
  // An item being started notices the flag once the start returns.
}

/** Queue an audio file import; returns the job id. */
export function enqueueImport(request: ImportRequest): string {
  void ensureBackgroundTranscriptionListeners();
  const id = `import-${Date.now()}-${++importSeq}`;
  importRequests.set(id, request);
  if (!imports.some((job) => isPendingStatus(job.status))) resetRun('import');
  imports = [...imports, { id, title: request.title, status: 'queued', stage: '', percentage: 0 }];
  publish();
  showProgress('import');
  pumpImports();
  return id;
}

export function cancelImport(id: string) {
  const job = imports.find((candidate) => candidate.id === id);
  if (!job || !isPendingStatus(job.status)) return;
  if (activeImport !== id) {
    settleImport(id, { status: 'cancelled' });
    return;
  }
  cancelledImport = id;
  void invoke('cancel_import_command').catch((error) => {
    console.error('[BackgroundTranscription] Failed to cancel import:', error);
  });
}

// --- Recording lane --------------------------------------------------------

function dispatchRecording(event: TranscriptionQueueEvent) {
  const wasIdle = recordings.order.length === 0;
  recordings = transcriptionQueueReducer(recordings, event);
  if (wasIdle && recordings.order.length > 0) resetRun('transcribe');
  publish();
  if (recordings.order.length > 0) showProgress('transcribe');
  const next = nextQueuedFolder(recordings);
  if (next) void runRecording(next);
}

function settleRecording(folderPath: string, event: TranscriptionQueueEvent) {
  const before = recordings.items[folderPath];
  recordings = transcriptionQueueReducer(recordings, event);
  const after = recordings.items[folderPath];
  publish();
  if (!before || !after || !isPendingStatus(before.status) || isPendingStatus(after.status)) return;

  const options = recordingOptions.get(folderPath);
  const title = options?.title ?? folderTitle(folderPath);
  const status = after.status as SettledJob['status'];

  if (status === 'done') {
    runs.transcribe.done.push({ title, meetingId: after.meetingId, segmentsCount: after.segmentsCount });
    if (options?.summarize && after.meetingId && typeof window !== 'undefined') {
      // The meeting page generates the summary when it next opens this meeting.
      window.sessionStorage.setItem(REQUESTED_SUMMARY_KEY, after.meetingId);
    }
  } else if (status === 'error') {
    runs.transcribe.failed.push({ title, message: after.errorMessage ?? '' });
  } else {
    toast.info(tr('library.transcribeCancelled'), { description: title });
  }

  emitSettled({
    kind: 'transcribe',
    status,
    title,
    folderPath,
    meetingId: after.meetingId,
    segmentsCount: after.segmentsCount,
    errorMessage: after.errorMessage,
  });

  if (recordings.order.length === 0) {
    void finishRun('transcribe');
    return;
  }
  showProgress('transcribe');
  const next = nextQueuedFolder(recordings);
  if (next) void runRecording(next);
}

async function readSpeechSettings(): Promise<ActiveSpeechSettings | null> {
  try {
    return await invoke<ActiveSpeechSettings>('api_get_active_speech_settings');
  } catch (error) {
    console.error('[BackgroundTranscription] Failed to read the speech settings:', error);
    return null;
  }
}

async function waitWhileBusy(command: string, isCancelled: () => boolean) {
  const deadline = Date.now() + BUSY_WAIT_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (isCancelled()) return;
    if (!(await invoke<boolean>(command))) return;
    await new Promise((resolve) => setTimeout(resolve, BUSY_POLL_MS));
  }
  throw new Error('Timed out waiting for the running transcription');
}

class Cancelled extends Error {}

async function runRecording(folderPath: string) {
  if (startingFolders.has(folderPath)) return;
  const item = recordings.items[folderPath];
  if (!item) return;

  startingFolders.add(folderPath);
  recordings = transcriptionQueueReducer(recordings, { type: 'activate', folderPath });
  publish();
  const isCancelled = () => cancelledFolders.has(folderPath);
  const options = recordingOptions.get(folderPath);

  try {
    let meetingId = item.meetingId;
    if (!meetingId) {
      // A deferred recording may have no meeting row yet; the transcript needs one.
      meetingId = await invoke<string>('ensure_meeting_for_recording', {
        folderPath,
        meetingTitle: options?.title ?? folderTitle(folderPath),
      });
      if (!meetingId) throw new Error('No meeting id was returned');
      recordings = transcriptionQueueReducer(recordings, { type: 'resolved', folderPath, meetingId });
      publish();
    }
    meetingToFolder.set(meetingId, folderPath);
    if (isCancelled()) throw new Cancelled();

    const args = {
      meetingId,
      meetingFolderPath: folderPath,
      ...transcribeArgs(await readSpeechSettings(), options?.model, options?.language),
    };
    try {
      await invoke('start_retranscription_command', args);
    } catch (error) {
      if (!errorMessage(error).toLowerCase().includes(BUSY_ERROR)) throw error;
      // Another job owns the backend slot: stay queued, wait, then start once more.
      dispatchRecording({ type: 'waiting', folderPath });
      await waitWhileBusy('is_retranscription_in_progress_command', isCancelled);
      if (isCancelled()) throw new Cancelled();
      await invoke('start_retranscription_command', args);
    }

    if (isCancelled()) {
      // Accepted by the backend, so stop it before the next start hits the guard.
      await invoke('cancel_retranscription_command').catch(() => undefined);
      throw new Cancelled();
    }
    dispatchRecording({ type: 'started', folderPath });
  } catch (error) {
    if (error instanceof Cancelled || isCancelled()) {
      settleRecording(folderPath, { type: 'cancelled', folderPath });
    } else {
      settleRecording(folderPath, { type: 'fail', folderPath, message: errorMessage(error) });
    }
  } finally {
    startingFolders.delete(folderPath);
    cancelledFolders.delete(folderPath);
  }
}

function pendingRecordingFor(meetingId: string | undefined): TranscriptionQueueItem | undefined {
  if (!meetingId) return undefined;
  const folder = meetingToFolder.get(meetingId);
  const item = folder ? recordings.items[folder] : undefined;
  return item && isPendingStatus(item.status) ? item : undefined;
}

// --- Import lane -----------------------------------------------------------

function patchImport(id: string, changes: Partial<ImportJob>) {
  imports = imports.map((job) => (job.id === id ? { ...job, ...changes } : job));
  publish();
}

function settleImport(id: string, changes: Partial<ImportJob> & { status: SettledJob['status'] }) {
  const job = imports.find((candidate) => candidate.id === id);
  if (!job || !isPendingStatus(job.status)) return;
  patchImport(id, { ...changes, stage: '' });
  importRequests.delete(id);
  if (activeImport === id) activeImport = null;
  if (cancelledImport === id) cancelledImport = null;

  if (changes.status === 'done') {
    runs.import.done.push({ title: job.title, meetingId: changes.meetingId, segmentsCount: changes.segmentsCount });
  } else if (changes.status === 'error') {
    runs.import.failed.push({ title: job.title, message: changes.errorMessage ?? '' });
  } else {
    toast.info(tr('bgTranscribe.importCancelled'), { description: job.title });
  }
  emitSettled({
    kind: 'import',
    status: changes.status,
    title: job.title,
    meetingId: changes.meetingId,
    errorMessage: changes.errorMessage,
  });

  if (imports.some((candidate) => isPendingStatus(candidate.status))) {
    showProgress('import');
    pumpImports();
  } else {
    // Settled jobs only matter until the lane drains; keep the list short.
    imports = [];
    publish();
    void finishRun('import');
  }
}

function pumpImports() {
  if (activeImport) return;
  const next = imports.find((job) => job.status === 'queued');
  if (next) void runImport(next.id);
}

async function runImport(id: string) {
  const request = importRequests.get(id);
  if (!request) return;
  activeImport = id;
  const isCancelled = () => cancelledImport === id;
  const args = {
    sourcePath: request.sourcePath,
    title: request.title,
    language: request.language || null,
    model: request.model || null,
    provider: request.provider || null,
  };

  try {
    try {
      await invoke('start_import_audio_command', args);
    } catch (error) {
      if (!errorMessage(error).toLowerCase().includes(BUSY_ERROR)) throw error;
      await waitWhileBusy('is_import_in_progress_command', isCancelled);
      if (isCancelled()) throw new Cancelled();
      await invoke('start_import_audio_command', args);
    }
    if (isCancelled()) {
      await invoke('cancel_import_command').catch(() => undefined);
      throw new Cancelled();
    }
    // Progress may already have marked it running; only fill in the gap.
    const job = imports.find((candidate) => candidate.id === id);
    if (job?.status === 'queued') patchImport(id, { status: 'running' });
  } catch (error) {
    if (error instanceof Cancelled || isCancelled()) settleImport(id, { status: 'cancelled' });
    else settleImport(id, { status: 'error', errorMessage: errorMessage(error) });
  }
}

// --- Events ----------------------------------------------------------------

interface ProgressPayload {
  meeting_id?: string;
  stage?: string;
  progress_percentage?: number;
  message?: string;
}

let listenersReady: Promise<void> | null = null;

/**
 * Subscribe to the backend's progress and result events once per window. Called
 * at app start by the provider, and again (as a no-op) before any job starts.
 */
export function ensureBackgroundTranscriptionListeners(): Promise<void> {
  if (!listenersReady) {
    listenersReady = (async () => {
      await listen<ProgressPayload>('retranscription-progress', (event) => {
        const item = pendingRecordingFor(event.payload?.meeting_id);
        if (!item) return;
        dispatchRecording({
          type: 'progress',
          folderPath: item.folderPath,
          stage: event.payload.stage ?? '',
          percentage: event.payload.progress_percentage ?? 0,
        });
      });
      await listen<{ meeting_id?: string; segments_count?: number }>('retranscription-complete', (event) => {
        const item = pendingRecordingFor(event.payload?.meeting_id);
        if (!item) return;
        cancelledFolders.delete(item.folderPath);
        settleRecording(item.folderPath, {
          type: 'complete',
          folderPath: item.folderPath,
          segmentsCount: event.payload.segments_count ?? 0,
        });
      });
      await listen<{ meeting_id?: string; error?: string }>('retranscription-error', (event) => {
        const item = pendingRecordingFor(event.payload?.meeting_id);
        if (!item) return;
        const message = event.payload.error ?? '';
        const cancelled = cancelledFolders.has(item.folderPath) || message.toLowerCase().includes('cancelled');
        cancelledFolders.delete(item.folderPath);
        settleRecording(
          item.folderPath,
          cancelled
            ? { type: 'cancelled', folderPath: item.folderPath }
            : { type: 'fail', folderPath: item.folderPath, message },
        );
      });

      // Import events carry no job id; the lane runs one import at a time, so
      // they belong to the active job.
      await listen<ProgressPayload>('import-progress', (event) => {
        if (!activeImport) return;
        patchImport(activeImport, {
          status: 'running',
          stage: event.payload?.stage ?? '',
          percentage: clampPercent(event.payload?.progress_percentage ?? 0),
        });
        showProgress('import');
      });
      await listen<{ meeting_id: string; segments_count?: number }>('import-complete', async (event) => {
        const id = activeImport;
        if (!id) return;
        const meetingId = event.payload?.meeting_id;
        if (meetingId) {
          try {
            await applyPinnedSummaryLanguageToMeeting(meetingId);
          } catch (error) {
            console.warn('[BackgroundTranscription] Could not apply the summary language:', error);
          }
        }
        settleImport(id, {
          status: 'done',
          meetingId,
          segmentsCount: event.payload?.segments_count,
          percentage: 100,
        });
      });
      await listen<{ error?: string }>('import-error', (event) => {
        const id = activeImport;
        if (!id) return;
        const message = event.payload?.error ?? '';
        const cancelled = cancelledImport === id || message.toLowerCase().includes('cancelled');
        settleImport(id, cancelled ? { status: 'cancelled' } : { status: 'error', errorMessage: message });
      });
    })().catch((error) => {
      console.error('[BackgroundTranscription] Failed to subscribe to transcription events:', error);
      listenersReady = null;
    });
  }
  return listenersReady;
}

function clampPercent(value: number): number {
  return Number.isFinite(value) ? Math.min(100, Math.max(0, Math.round(value))) : 0;
}

function emitSettled(job: SettledJob) {
  settledListeners.forEach((listener) => {
    try {
      listener(job);
    } catch (error) {
      console.error('[BackgroundTranscription] Settled listener failed:', error);
    }
  });
  if (job.status !== 'cancelled' && job.meetingId) void host?.refreshMeetings?.();
}

// --- Presentation ----------------------------------------------------------

function resetRun(kind: SettledJob['kind']) {
  runs[kind] = { done: [], failed: [] };
  progressHidden[kind] = false;
  lastProgressText[kind] = '';
  runGeneration[kind] += 1;
}

function stageLabel(stage: string): string {
  const key = transcriptionStageKey(stage);
  return key ? tr(key) : stage;
}

/** Keep the single progress toast for a lane in step with its active job. */
function showProgress(kind: SettledJob['kind']) {
  if (progressHidden[kind]) return;

  let title: string;
  let detail: string;
  let remaining: number;
  let cancel: () => void;

  if (kind === 'transcribe') {
    const folder = recordings.active ?? recordings.order[0];
    const item = folder ? recordings.items[folder] : undefined;
    if (!folder || !item) return;
    title = tr('bgTranscribe.running', { name: recordingOptions.get(folder)?.title ?? folderTitle(folder) });
    detail = item.waitingForBusy
      ? tr('library.transcribeBusy')
      : item.status === 'running'
        ? `${stageLabel(item.stage) || tr('library.stage.transcribing')} · ${item.percentage}%`
        : tr('library.transcribeQueued');
    remaining = recordings.order.length - 1;
    cancel = () => cancelTranscription(folder);
  } else {
    const job = imports.find((candidate) => candidate.id === activeImport)
      ?? imports.find((candidate) => candidate.status === 'queued');
    if (!job) return;
    title = tr('bgTranscribe.importing', { name: job.title });
    detail = job.status === 'running'
      ? `${stageLabel(job.stage) || tr('library.stage.transcribing')} · ${job.percentage}%`
      : tr('library.transcribeQueued');
    remaining = imports.filter((candidate) => isPendingStatus(candidate.status)).length - 1;
    cancel = () => cancelImport(job.id);
  }

  const description = remaining > 0 ? `${detail} · ${tr('bgTranscribe.queuedMore', { count: remaining })}` : detail;
  const text = `${title}\n${description}`;
  if (text === lastProgressText[kind]) return;
  lastProgressText[kind] = text;
  const generation = runGeneration[kind];

  toast.loading(title, {
    id: TOAST_ID[kind],
    description,
    action: { label: tr('common.cancel'), onClick: cancel },
    // Closing it only hides progress; the job keeps running and still reports.
    // Sonner also reports the programmatic dismiss at the end of a run, after
    // the next run may have started; only a dismissal of this run counts.
    onDismiss: () => {
      if (runGeneration[kind] === generation) progressHidden[kind] = true;
    },
  });
}

async function windowIsFocused(): Promise<boolean> {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    return await getCurrentWindow().isFocused();
  } catch {
    return typeof document !== 'undefined' ? document.hasFocus() : true;
  }
}

/**
 * Tell the user, through the app's notification settings, that background work
 * finished. Clicking a desktop notification brings the app to the front, where
 * the completion toast waits with its "View" button.
 */
async function notifySystem(title: string, body: string) {
  try {
    await invoke('show_notification', {
      notification: {
        id: null,
        title,
        body,
        notification_type: 'TranscriptionComplete',
        priority: 'Normal',
        timeout: 'Default',
        icon: null,
        sound: true,
        actions: [],
      },
    });
  } catch (error) {
    // The manager starts lazily; fall back to the plugin so the alert is not lost.
    try {
      const { isPermissionGranted, sendNotification } = await import('@tauri-apps/plugin-notification');
      if (await isPermissionGranted()) sendNotification({ title, body });
    } catch (fallbackError) {
      console.error('[BackgroundTranscription] Could not show a notification:', error, fallbackError);
    }
  }
}

async function finishRun(kind: SettledJob['kind']) {
  toast.dismiss(TOAST_ID[kind]);
  const announcement = describeRun(kind, runs[kind]);
  resetRun(kind);
  if (!announcement) return;

  const focused = await windowIsFocused();
  const show = announcement.kind === 'success' ? toast.success : announcement.kind === 'partial' ? toast.warning : toast.error;
  show(announcement.title, {
    id: `${TOAST_ID[kind]}-result`,
    description: announcement.description,
    action: {
      label: tr('bgTranscribe.view'),
      onClick: () => {
        if (host) host.navigate(announcement.href);
        else if (typeof window !== 'undefined') window.location.assign(announcement.href);
      },
    },
    // A user who was away should still find the result when they come back.
    duration: focused ? 10_000 : Infinity,
  });

  if (!focused) {
    void notifySystem(announcement.title, announcement.description ?? tr('bgTranscribe.notifyBody'));
  }
}
