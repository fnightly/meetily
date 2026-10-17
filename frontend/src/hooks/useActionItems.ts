'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ActionItem, ActionItemDraft } from '@/lib/action-items';

export const ACTION_ITEMS_EVENT = 'meetily:action-items-changed';

function announce() {
  window.dispatchEvent(new CustomEvent(ACTION_ITEMS_EVENT));
}

/** Action items for one meeting, or across all meetings when `meetingId` is null. */
export function useActionItems(meetingId: string | null) {
  const [items, setItems] = useState<ActionItem[]>([]);
  const [loading, setLoading] = useState(true);

  const reload = useCallback(async () => {
    try {
      const list = await invoke<ActionItem[]>('api_list_action_items', { meetingId, includeDone: true });
      setItems(list);
    } catch {
      setItems([]);
    } finally {
      setLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    void reload();
    const onChange = () => void reload();
    window.addEventListener(ACTION_ITEMS_EVENT, onChange);
    return () => window.removeEventListener(ACTION_ITEMS_EVENT, onChange);
  }, [reload]);

  const create = useCallback(async (draft: ActionItemDraft, forMeeting: string | null = meetingId) => {
    await invoke('api_create_action_item', { meetingId: forMeeting, item: draft });
    announce();
  }, [meetingId]);

  const update = useCallback(async (id: string, patch: Partial<Pick<ActionItem, 'text' | 'owner' | 'due_date' | 'done'>>) => {
    // Optimistic: ticking a box should feel instant.
    setItems((prev) => prev.map((it) => (it.id === id ? { ...it, ...patch } as ActionItem : it)));
    await invoke('api_update_action_item', {
      id,
      patch: {
        text: patch.text,
        owner: patch.owner === null ? '' : patch.owner,
        due_date: patch.due_date === null ? '' : patch.due_date,
        done: patch.done,
      },
    });
    announce();
  }, []);

  const remove = useCallback(async (id: string) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
    await invoke('api_delete_action_item', { id });
    announce();
  }, []);

  const importDrafts = useCallback(async (forMeeting: string, drafts: ActionItemDraft[], force: boolean) => {
    const outcome = await invoke<{ imported: number; skipped: boolean }>('api_import_action_items', {
      meetingId: forMeeting,
      items: drafts,
      force,
    });
    if (outcome.imported > 0) announce();
    return outcome;
  }, []);

  return { items, loading, reload, create, update, remove, importDrafts };
}
