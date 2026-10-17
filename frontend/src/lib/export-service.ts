/**
 * Loads meetings from the backend and saves them in any export format.
 *
 * Kept apart from `export-formats.ts` (pure rendering) because this module
 * talks to Tauri.
 */

import { invoke } from '@tauri-apps/api/core';
import type { MeetingSummary, SummaryProcessResponse } from '@/types';
import { parseSummaryContent } from './summary-content';
import { displaySpeaker, fetchSpeakerNames } from './speaker-names';
import type { ExportLabels, ExportScope, ExportSegment, MeetingExportInput } from './meeting-export';
import { buildExportFile, type ExportFilePayload, type ExportFormat } from './export-formats';

type Translate = (key: string, params?: Record<string, string | number>) => string;

interface MeetingDetails {
  id: string;
  title: string;
  created_at: string;
  updated_at?: string;
  transcripts: ExportSegment[];
}

export interface ExportResult {
  paths: string[];
  folder: string | null;
  cancelled: boolean;
}

export const EXPORT_FORMAT_STORAGE_KEY = 'meetily.export.format';
export const EXPORT_SCOPE_STORAGE_KEY = 'meetily.export.scope';

export function exportLabels(t: Translate): ExportLabels {
  return {
    untitled: t('export.label.untitled'),
    date: t('export.label.date'),
    meetingId: t('export.label.meetingId'),
    segments: t('export.label.segments'),
    duration: t('export.label.duration'),
    summary: t('export.label.summary'),
    transcript: t('export.label.transcript'),
    noSummary: t('export.label.noSummary'),
    noTranscript: t('export.label.noTranscript'),
  };
}

/** Read one meeting completely: all transcript segments (not one page) and its summary. */
export async function loadMeetingForExport(
  meetingId: string,
  t: Translate,
  summaryOverride?: MeetingSummary | null,
): Promise<MeetingExportInput> {
  const details = await invoke<MeetingDetails>('api_get_meeting', { meetingId });

  let summary: MeetingSummary | null | undefined = summaryOverride;
  if (summary === undefined) {
    try {
      const response = await invoke<SummaryProcessResponse>('api_get_summary', { meetingId });
      summary = response.status === 'idle' ? null : parseSummaryContent(response.data);
    } catch {
      summary = null;
    }
  }

  const speakerNames = await fetchSpeakerNames(meetingId);

  return {
    meeting: {
      id: details.id,
      title: details.title,
      created_at: details.created_at,
      updated_at: details.updated_at,
    },
    summary,
    transcripts: details.transcripts ?? [],
    speakerLabel: (raw: string) => displaySpeaker(raw, t, speakerNames),
  };
}

export interface ExportJob {
  meetingIds: string[];
  format: ExportFormat;
  scope: ExportScope;
  t: Translate;
  locale: string;
  /** Live summary from an open editor, used for a single-meeting export. */
  summaryOverride?: MeetingSummary | null;
  onProgress?: (done: number, total: number) => void;
}

/**
 * Render every meeting, then hand all files to the backend in one call:
 * a single meeting opens "Save as", several open a folder picker.
 * Meetings that fail to load are reported, the rest are still exported.
 */
export async function runExport(job: ExportJob): Promise<ExportResult & { failed: string[] }> {
  const labels = exportLabels(job.t);
  const files: ExportFilePayload[] = [];
  const failed: string[] = [];
  const total = job.meetingIds.length;

  for (let i = 0; i < total; i++) {
    const id = job.meetingIds[i];
    try {
      const input = await loadMeetingForExport(
        id,
        job.t,
        total === 1 ? job.summaryOverride : undefined,
      );
      files.push(buildExportFile(input, job.format, job.scope, labels, job.locale));
    } catch (error) {
      console.error(`Export: failed to load meeting ${id}`, error);
      failed.push(id);
    }
    job.onProgress?.(i + 1, total);
  }

  if (files.length === 0) {
    throw new Error(job.t('export.nothingLoaded'));
  }

  const result = await invoke<ExportResult>('export_save_files', {
    files,
    mode: total === 1 ? 'file' : 'folder',
    dialogTitle: total === 1 ? job.t('export.saveDialogTitle') : job.t('export.folderDialogTitle'),
  });
  return { ...result, failed };
}

export async function revealExportPath(path: string): Promise<void> {
  await invoke('reveal_export_path', { path });
}
