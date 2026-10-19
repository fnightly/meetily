'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ActionItem, ActionItemDraft } from '@/lib/action-items';

export const ACTION_ITEMS_EVENT = 'meetily:action-items-changed';

/**
 * Announce a change to every other `useActionItems` consumer. The source id lets
 * the hook that caused the change skip its own event: its optimistic update (or
 * the explicit reload after a create) already reflects the new state, so
 * re-fetching on its own event would be a duplicate request.
 */
function announce(source?: string) {
  window.dispatchEvent(new CustomEvent(ACTION_ITEMS_EVENT, source ? { detail: { source } } : undefined));
}

// The workspace panel and the meeting pane mount together and ask for the same
// list. Share the in-flight request so that is one query, not two.
const inFlight = new Map<string, Promise<ActionItem[]>>();

function listKey(meetingId: string | null): string {
  return meetingId ?? '*';
}

function fetchItems(meetingId: string | null): Promise<ActionItem[]> {
  const key = listKey(meetingId);
  const existing = inFlight.get(key);
  if (existing) return existing;
  let request: Promise<ActionItem[]>;
  request = invoke<ActionItem[]>('api_list_action_items', { meetingId, includeDone: true })
    .finally(() => {
      // Drop only the entry this call installed; a later reload may have replaced it.
      if (inFlight.get(key) === request) inFlight.delete(key);
    });
  inFlight.set(key, request);
  return request;
}

let nextSourceId = 0;

/** Action items for one meeting, or across all meetings when `meetingId` is null. */
export function useActionItems(meetingId: string | null) {
  const [items, setItems] = useState<ActionItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Stable identity for this hook instance, so its own change events are ignored.
  const [sourceId] = useState(() => `action-items-${++nextSourceId}`);

  const mountedRef = useRef(true);
  const requestSeqRef = useRef(0);

  const reload = useCallback(async () => {
    // Later requests (a meeting change, another consumer's event) win; an older
    // response that lands late must not overwrite newer rows.
    const seq = ++requestSeqRef.current;
    try {
      const list = await fetchItems(meetingId);
      if (seq !== requestSeqRef.current || !mountedRef.current) return;
      setItems(list);
      setError(null);
    } catch (e) {
      if (seq !== requestSeqRef.current || !mountedRef.current) return;
      // Keep the rows already on screen: a transient backend failure should not
      // turn a populated list into the empty state.
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (seq === requestSeqRef.current && mountedRef.current) setLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    mountedRef.current = true;
    void reload();
    const onChange = (event: Event) => {
      const source = (event as CustomEvent<{ source?: string } | undefined>).detail?.source;
      if (source === sourceId) return;
      void reload();
    };
    window.addEventListener(ACTION_ITEMS_EVENT, onChange);
    return () => {
      mountedRef.current = false;
      window.removeEventListener(ACTION_ITEMS_EVENT, onChange);
    };
  }, [reload, sourceId]);

  const create = useCallback(async (draft: ActionItemDraft, forMeeting: string | null = meetingId) => {
    await invoke('api_create_action_item', { meetingId: forMeeting, item: draft });
    announce(sourceId);
    // The backend assigns the id and timestamps; read the canonical row back.
    if (forMeeting === meetingId) await reload();
  }, [meetingId, reload, sourceId]);

  const update = useCallback(async (id: string, patch: Partial<Pick<ActionItem, 'text' | 'owner' | 'due_date' | 'done'>>) => {
    // Optimistic: ticking a box should feel instant.
    setItems((prev) => prev.map((it) => (it.id === id ? { ...it, ...patch } as ActionItem : it)));
    try {
      await invoke('api_update_action_item', {
        id,
        patch: {
          text: patch.text,
          owner: patch.owner === null ? '' : patch.owner,
          due_date: patch.due_date === null ? '' : patch.due_date,
          done: patch.done,
        },
      });
    } catch (e) {
      // The optimistic rows are now wrong; read the stored state back.
      await reload();
      throw e;
    }
    announce(sourceId);
  }, [reload, sourceId]);

  const remove = useCallback(async (id: string) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
    try {
      await invoke('api_delete_action_item', { id });
    } catch (e) {
      await reload();
      throw e;
    }
    announce(sourceId);
  }, [reload, sourceId]);

  const importDrafts = useCallback(async (forMeeting: string, drafts: ActionItemDraft[], force: boolean) => {
    const outcome = await invoke<{ imported: number; skipped: boolean }>('api_import_action_items', {
      meetingId: forMeeting,
      items: drafts,
      force,
    });
    if (outcome.imported > 0) {
      announce(sourceId);
      await reload();
    }
    return outcome;
  }, [reload, sourceId]);

  return { items, loading, error, reload, create, update, remove, importDrafts };
}
