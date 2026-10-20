"use client";

import { useCallback, useEffect, useImperativeHandle, useRef, useState, type MutableRefObject } from 'react';
import dynamic from 'next/dynamic';
import { Block } from '@blocknote/core';
import { useCreateBlockNote } from '@blocknote/react';
import { BlockNoteView } from '@blocknote/shadcn';
import type { BlockNoteBlock } from '@/types';
import { blocksToMarkdownSafely } from '@/lib/blocknote-markdown';
import "@blocknote/shadcn/style.css";

/**
 * The heavy half of `BlockNoteSummaryView`.
 *
 * Everything that statically imports BlockNote/Tiptap/ProseMirror lives here, so
 * the parent shell can pull it in with `next/dynamic` and keep the editor out of
 * the route's initial JavaScript. The shell keeps the public ref contract and
 * forwards it through `controlRef` once this surface has mounted.
 */

// Dynamically import BlockNote Editor to avoid SSR issues
const Editor = dynamic(() => import('../BlockNoteEditor/Editor'), { ssr: false });

export interface BlockNoteSummarySurfaceHandle {
  saveSummary: () => Promise<void>;
  getMarkdown: () => Promise<string>;
}

export interface BlockNoteSummarySurfaceProps {
  /** Detected summary format; only the editor-backed formats reach this surface. */
  format: 'blocknote' | 'markdown';
  /** Raw summary payload as detected by the shell. */
  data: any;
  onSave?: (data: { markdown?: string; summary_json?: BlockNoteBlock[] }) => Promise<void>;
  onDirtyChange?: (isDirty: boolean) => void;
  /** Receives the imperative handle the shell exposes through its forwarded ref. */
  controlRef: MutableRefObject<BlockNoteSummarySurfaceHandle | null>;
}

export function BlockNoteSummarySurface({
  format,
  data,
  onSave,
  onDirtyChange,
  controlRef,
}: BlockNoteSummarySurfaceProps) {
  const [isDirty, setIsDirty] = useState(false);
  const [currentBlocks, setCurrentBlocks] = useState<Block[]>([]);
  const [, setIsSaving] = useState(false);
  const isContentLoaded = useRef(false);

  // Create BlockNote editor for markdown parsing
  const editor = useCreateBlockNote({
    initialContent: undefined
  });

  // Parse markdown to blocks when format is markdown
  useEffect(() => {
    if (format === 'markdown' && data?.markdown && editor) {
      const loadMarkdown = async () => {
        try {
          console.log('📝 Parsing markdown to BlockNote blocks...');
          const blocks = await editor.tryParseMarkdownToBlocks(data.markdown);
          editor.replaceBlocks(editor.document, blocks);
          console.log('✅ Markdown parsed successfully');

          // Delay to ensure editor has finished rendering before allowing onChange
          setTimeout(() => {
            isContentLoaded.current = true;
          }, 100);
        } catch (err) {
          console.error('❌ Failed to parse markdown:', err);
        }
      };
      loadMarkdown();
    }
  }, [format, data?.markdown, editor]);

  // Set content loaded flag for blocknote format
  useEffect(() => {
    if (format === 'blocknote' && data?.summary_json) {
      // Delay to ensure editor has finished rendering
      setTimeout(() => {
        isContentLoaded.current = true;
      }, 100);
    }
  }, [format, data?.summary_json]);

  const handleEditorChange = useCallback((blocks: Block[]) => {
    // Only set dirty flag if content has finished loading
    if (isContentLoaded.current) {
      setCurrentBlocks(blocks);
      setIsDirty(true);
    }
  }, []);

  // Notify parent of dirty state changes
  useEffect(() => {
    if (onDirtyChange) {
      onDirtyChange(isDirty);
    }
  }, [isDirty, onDirtyChange]);

  const handleSave = useCallback(async () => {
    if (!onSave || !isDirty) return;

    setIsSaving(true);
    try {
      console.log('💾 Saving BlockNote content...');

      // Generate markdown from current blocks; preserve BlockNote JSON even if markdown conversion fails.
      const markdownResult = await blocksToMarkdownSafely(editor, currentBlocks, {
        source: 'BlockNoteSummaryView.handleSave',
      });

      const saveData: { markdown?: string; summary_json?: BlockNoteBlock[] } = {
        summary_json: currentBlocks as unknown as BlockNoteBlock[]
      };

      if (markdownResult.markdown !== undefined) {
        saveData.markdown = markdownResult.markdown;
      }

      await onSave(saveData);
      setIsDirty(false);
      console.log('✅ Save successful');
    } catch (error) {
      console.error('❌ Save failed:', error);
      throw error;
    } finally {
      setIsSaving(false);
    }
  }, [onSave, isDirty, currentBlocks, editor]);

  const getMarkdown = useCallback(async () => {
    try {
      console.log('🔍 getMarkdown called, format:', format);
      console.log('🔍 currentBlocks length:', currentBlocks.length);
      console.log('🔍 data:', data);

      // For markdown format - use the main editor
      if (format === 'markdown' && editor) {
        console.log('📝 Using markdown editor, blocks:', editor.document.length);
        const markdownResult = await blocksToMarkdownSafely(editor, editor.document, {
          source: 'BlockNoteSummaryView.getMarkdown.markdown',
          fallbackMarkdown: data?.markdown,
        });
        console.log('📝 Generated markdown length:', markdownResult.markdown?.length || 0);
        return markdownResult.markdown || '';
      }

      // For blocknote format - use currentBlocks state
      if (format === 'blocknote') {
        console.log('📝 BlockNote format, currentBlocks:', currentBlocks.length);
        const blocks = currentBlocks.length > 0
          ? currentBlocks
          : (data?.summary_json as unknown as Block[] | undefined) || [];

        if (blocks.length > 0 && editor) {
          const markdownResult = await blocksToMarkdownSafely(editor, blocks, {
            source: 'BlockNoteSummaryView.getMarkdown.blocknote',
            fallbackMarkdown: data?.markdown,
          });
          console.log('📝 Generated markdown from blocks, length:', markdownResult.markdown?.length || 0);
          return markdownResult.markdown || '';
        }
        // Fallback: if we have the original data with markdown
        if (data?.markdown) {
          console.log('📝 Using fallback markdown from data');
          return data.markdown;
        }
      }

      console.warn('⚠️ Cannot generate markdown, returning empty');
      return '';
    } catch (err) {
      console.error('❌ Failed to generate markdown:', err);
      return '';
    }
  }, [format, currentBlocks, editor, data]);

  // Expose methods to the shell, which forwards them through the public ref.
  useImperativeHandle(controlRef, () => ({
    saveSummary: handleSave,
    getMarkdown,
  }), [handleSave, getMarkdown]);

  // Render BlockNote format (has summary_json)
  if (format === 'blocknote') {
    console.log('🎨 Rendering BLOCKNOTE format (direct)');
    return (
      <div className="flex flex-col w-full">
        <div className="w-full">
          <Editor
            initialContent={data?.summary_json}
            onChange={(blocks) => {
              console.log('📝 Editor blocks changed:', blocks.length);
              handleEditorChange(blocks);
            }}
            editable={true}
          />
        </div>
      </div>
    );
  }

  // Render Markdown format (parse and display in BlockNote)
  console.log('🎨 Rendering MARKDOWN format (parsed to BlockNote)');
  return (
    <div className="flex flex-col w-full">
      <div className="w-full">
        <BlockNoteView
          editor={editor}
          editable={true}
          onChange={() => {
            if (isContentLoaded.current) {
              handleEditorChange(editor.document);
            }
          }}
          theme="light"
        />
      </div>
    </div>
  );
}
