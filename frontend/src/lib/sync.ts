/**
 * Client for the Obsidian / Notion sync commands.
 *
 * The note itself is built with `buildMeetingMarkdown`, the same renderer the
 * Markdown export uses: Obsidian notes are Markdown files, and the Notion page
 * is derived from the same text. Keeping one builder means "what is in the
 * exported file" and "what is synced" cannot drift apart.
 */

import { invoke } from '@tauri-apps/api/core';
import type { MeetingSummary } from '@/types';
import { buildMeetingMarkdown, summaryToMarkdown } from './meeting-export';
import { exportLabels, loadMeetingForExport } from './export-service';

type Translate = (key: string, params?: Record<string, string | number>) => string;

/** Mirrors `sync::settings::SyncSettings`. */
export interface SyncSettings {
  obsidian_folder: string | null;
  obsidian_subfolder: string | null;
  obsidian_template: string | null;
  obsidian_tags: string[];
  notion_database_id: string | null;
}

/** Mirrors `sync::settings::SyncSettingsView`. */
export interface SyncSettingsView {
  settings: SyncSettings;
  /** The notes folder from storage settings, used when no folder is chosen. */
  default_obsidian_folder: string;
  effective_obsidian_folder: string;
  /** Where notes are actually written, including the subfolder. */
  obsidian_notes_folder: string;
  /** Whether a token is stored. The token itself is never returned. */
  notion_token_set: boolean;
}

/** Mirrors `sync::commands::SyncNoteRequest`. */
export interface SyncNotePayload {
  meeting_id: string;
  title: string;
  date: string;
  markdown: string;
  include_audio?: boolean;
}

export type ObsidianWriteStatus = 'created' | 'unchanged' | 'alternative';

/** Mirrors `sync::obsidian::ObsidianWriteOutcome`. */
export interface ObsidianSyncResult {
  path: string;
  status: ObsidianWriteStatus;
  existing_path: string | null;
}

/** Mirrors `sync::commands::NotionSyncResult`. */
export interface NotionSyncResult {
  page_id: string;
  page_url: string | null;
  blocks_written: number;
  database_title: string | null;
  data_source_name: string | null;
  audio_uploaded?: boolean;
  audio_skipped_reason?: string | null;
}

/** Mirrors `sync::commands::NotionTestResult`. */
export interface NotionTestResult {
  database_title: string | null;
  data_source_name: string | null;
  title_property: string;
}

export interface PreparedSyncNote {
  payload: SyncNotePayload;
  hasSummary: boolean;
  hasTranscript: boolean;
}

export async function getSyncSettings(): Promise<SyncSettingsView> {
  return invoke<SyncSettingsView>('get_sync_settings');
}

/**
 * Save the settings.
 *
 * `notionToken` is omitted to keep the stored token, and an empty string to
 * remove it. A new token is written through the backend's API-key storage and is
 * never read back.
 */
export async function saveSyncSettings(
  settings: SyncSettings,
  notionToken?: string,
): Promise<SyncSettingsView> {
  return invoke<SyncSettingsView>('save_sync_settings', {
    settings,
    notionToken: notionToken ?? null,
  });
}

export async function pickObsidianFolder(): Promise<string | null> {
  return invoke<string | null>('pick_obsidian_folder');
}

export async function syncMeetingToObsidian(
  payload: SyncNotePayload,
): Promise<ObsidianSyncResult> {
  return invoke<ObsidianSyncResult>('obsidian_sync_meeting', { request: payload });
}

export async function syncMeetingToNotion(payload: SyncNotePayload): Promise<NotionSyncResult> {
  return invoke<NotionSyncResult>('notion_sync_meeting', { request: payload });
}

export async function testNotionConnection(): Promise<NotionTestResult> {
  return invoke<NotionTestResult>('notion_test_connection');
}

/**
 * Parse the comma-separated tags field.
 *
 * Mirrors the backend sanitiser: leading `#`, blanks and case-insensitive
 * duplicates are dropped, and both ASCII and Chinese separators work. Commas are
 * the documented format, but pasted lines are accepted too.
 */
export function parseSyncTags(raw: string): string[] {
  const seen = new Set<string>();
  const tags: string[] = [];

  for (const piece of raw.split(/[,，;；\n\r]+/)) {
    const tag = piece.trim().replace(/^#+/, '').trim();
    if (!tag) continue;

    const key = tag.toLowerCase();
    if (seen.has(key)) continue;

    seen.add(key);
    tags.push(Array.from(tag).slice(0, 64).join(''));
    if (tags.length >= 20) break;
  }

  return tags;
}

/** Render stored tags into the editable comma-separated field. */
export function formatSyncTags(tags: string[] | undefined): string {
  return (tags ?? []).join(', ');
}

/** Load one meeting and build the payload both targets accept. */
export async function prepareSyncNote(
  meetingId: string,
  t: Translate,
  summaryOverride?: MeetingSummary | null,
): Promise<PreparedSyncNote> {
  const input = await loadMeetingForExport(meetingId, t, summaryOverride);
  const hasTranscript = (input.transcripts?.length ?? 0) > 0;
  // A summary-only meeting should not gain a "no transcript yet" placeholder in
  // the note it pushes to Obsidian or Notion.
  const markdown = buildMeetingMarkdown(input, hasTranscript ? 'both' : 'summary', exportLabels(t));

  return {
    payload: {
      meeting_id: meetingId,
      title: input.meeting.title ?? '',
      // `created_at` may carry a time zone; the backend only keeps its date.
      date: input.meeting.created_at ?? '',
      markdown,
    },
    hasSummary: summaryToMarkdown(input.summary).trim().length > 0,
    hasTranscript,
  };
}
