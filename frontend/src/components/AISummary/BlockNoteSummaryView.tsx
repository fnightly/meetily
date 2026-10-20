"use client";

import { forwardRef, useCallback, useImperativeHandle, useRef, useState } from 'react';
import dynamic from 'next/dynamic';
import type { Summary, SummaryDataResponse, SummaryFormat, BlockNoteBlock } from '@/types';
import { AISummary } from './index';
import type { BlockNoteSummarySurfaceHandle } from './BlockNoteSummarySurface';

/**
 * The editor shell is deliberately light: it only decides which renderer the
 * payload needs and keeps the public ref contract. BlockNote/Tiptap/ProseMirror
 * (and their CSS) live in `BlockNoteSummarySurface`, which is loaded on demand,
 * so a route that renders a summary is not blocked on ~1.6 MB of editor code.
 */
const BlockNoteSummarySurface = dynamic(
  () => import('./BlockNoteSummarySurface').then((mod) => mod.BlockNoteSummarySurface),
  {
    ssr: false,
    loading: () => <BlockNoteSummarySkeleton />,
  },
);

/** Placeholder with the same block rhythm as the loaded editor. */
function BlockNoteSummarySkeleton() {
  return (
    <div role="status" aria-busy="true" className="w-full animate-pulse space-y-3 py-2">
      <div className="h-4 w-1/3 rounded bg-slate-200" />
      <div className="h-3 w-full rounded bg-slate-100" />
      <div className="h-3 w-5/6 rounded bg-slate-100" />
      <div className="h-3 w-2/3 rounded bg-slate-100" />
    </div>
  );
}

export interface BlockNoteSummaryViewProps {
  summaryData: SummaryDataResponse | Summary | null;
  onSave?: (data: { markdown?: string; summary_json?: BlockNoteBlock[] }) => Promise<void>;
  onSummaryChange?: (summary: Summary) => void;
  status?: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  error?: string | null;
  onRegenerateSummary?: () => void;
  meeting?: {
    id: string;
    title: string;
    created_at: string;
  };
  onDirtyChange?: (isDirty: boolean) => void;
}

export interface BlockNoteSummaryViewRef {
  saveSummary: () => Promise<void>;
  getMarkdown: () => Promise<string>;
  isDirty: boolean;
}

// Format detection helper
function detectSummaryFormat(data: any): { format: SummaryFormat; data: any } {
  if (!data) {
    return { format: 'legacy', data: null };
  }

  // Priority 1: BlockNote format (has summary_json)
  if (data.summary_json && Array.isArray(data.summary_json)) {
    console.log('✅ FORMAT: BLOCKNOTE (summary_json exists)');
    return { format: 'blocknote', data };
  }

  // Priority 2: Markdown format
  if (data.markdown && typeof data.markdown === 'string') {
    console.log('✅ FORMAT: MARKDOWN (will parse to BlockNote)');
    return { format: 'markdown', data };
  }

  // Priority 3: Legacy JSON
  const hasLegacyStructure = data.MeetingName || Object.keys(data).some(key =>
    typeof data[key] === 'object' && data[key]?.title && data[key]?.blocks
  );

  if (hasLegacyStructure) {
    console.log('✅ FORMAT: LEGACY (custom JSON)');
    return { format: 'legacy', data };
  }

  return { format: 'legacy', data: null };
}

export const BlockNoteSummaryView = forwardRef<BlockNoteSummaryViewRef, BlockNoteSummaryViewProps>(({
  summaryData,
  onSave,
  onSummaryChange,
  status = 'idle',
  error = null,
  onRegenerateSummary,
  meeting,
  onDirtyChange
}, ref) => {
  const { format, data } = detectSummaryFormat(summaryData);
  const [isDirty, setIsDirty] = useState(false);
  const surfaceRef = useRef<BlockNoteSummarySurfaceHandle | null>(null);

  // Until the editor chunk has loaded there is nothing editable, so reads fall
  // back to the markdown that came with the summary payload.
  const fallbackMarkdown = typeof data?.markdown === 'string' ? data.markdown : '';

  const handleDirtyChange = useCallback((dirty: boolean) => {
    setIsDirty(dirty);
    onDirtyChange?.(dirty);
  }, [onDirtyChange]);

  const saveSummary = useCallback(async () => {
    // Not dirty (or still loading) means there is nothing to persist.
    await surfaceRef.current?.saveSummary();
  }, []);

  const getMarkdown = useCallback(async () => {
    const surface = surfaceRef.current;
    if (surface) return surface.getMarkdown();
    return fallbackMarkdown;
  }, [fallbackMarkdown]);

  // Expose methods to parent via ref, preserving the previous contract.
  useImperativeHandle(ref, () => ({
    saveSummary,
    getMarkdown,
    isDirty,
  }), [saveSummary, getMarkdown, isDirty]);

  // Render legacy format
  if (format === 'legacy') {
    console.log('🎨 Rendering LEGACY format');
    return (
      <AISummary
        summary={summaryData as Summary}
        status={status}
        error={error}
        onSummaryChange={onSummaryChange || (() => { })}
        onRegenerateSummary={onRegenerateSummary || (() => { })}
        meeting={meeting}
      />
    );
  }

  return (
    <BlockNoteSummarySurface
      format={format}
      data={data}
      onSave={onSave}
      onDirtyChange={handleDirtyChange}
      controlRef={surfaceRef}
    />
  );
});

BlockNoteSummaryView.displayName = 'BlockNoteSummaryView';
