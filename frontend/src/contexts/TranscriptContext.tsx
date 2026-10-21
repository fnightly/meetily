'use client';

import React, { createContext, useContext, useState, useEffect, useRef, useCallback, ReactNode, MutableRefObject } from 'react';
import { Transcript, TranscriptUpdate } from '@/types';
import { toast } from 'sonner';
import { useRecordingState } from './RecordingStateContext';
import { transcriptService } from '@/services/transcriptService';
import { recordingService } from '@/services/recordingService';
import { indexedDBService } from '@/services/indexedDBService';
import { tr } from '@/lib/i18n';
import { isFinishedSession } from '@/lib/background-finalize';

interface TranscriptContextType {
  transcripts: Transcript[];
  transcriptsRef: MutableRefObject<Transcript[]>
  addTranscript: (update: TranscriptUpdate) => void;
  copyTranscript: () => void;
  flushBuffer: () => void;
  transcriptContainerRef: React.RefObject<HTMLDivElement>;
  meetingTitle: string;
  setMeetingTitle: (title: string) => void;
  clearTranscripts: () => void;
  currentMeetingId: string | null;
  /** Mark a recovery entry saved; defaults to the current recording's. */
  markMeetingAsSaved: (meetingId?: string | null) => Promise<void>;
}

const TranscriptContext = createContext<TranscriptContextType | undefined>(undefined);

/** Ordering for the live transcript list: recording time first, then arrival order. */
export function compareTranscripts(a: Transcript, b: Transcript): number {
  const chunkTimeDiff = (a.chunk_start_time || 0) - (b.chunk_start_time || 0);
  return chunkTimeDiff !== 0 ? chunkTimeDiff : (a.sequence_id || 0) - (b.sequence_id || 0);
}

/**
 * Merge newly arrived segments into the ordered transcript list.
 *
 * Live updates arrive almost in order, so the common case is an append. Only a
 * genuinely out-of-order batch pays for a linear merge; the full array is never
 * re-sorted. Segments without a sequence id are ignored here (live updates always
 * carry one) and duplicates are dropped by sequence id, preserving the previous
 * duplicate protection. Returns the same array when nothing is new so React can
 * skip the re-render.
 */
export function mergeTranscripts(existing: Transcript[], incoming: Transcript[]): Transcript[] {
  const seen = new Set<number>();
  for (const transcript of existing) {
    if (transcript.sequence_id !== undefined) seen.add(transcript.sequence_id);
  }

  const unique: Transcript[] = [];
  for (const transcript of incoming) {
    if (transcript.sequence_id === undefined || seen.has(transcript.sequence_id)) continue;
    seen.add(transcript.sequence_id);
    unique.push(transcript);
  }
  if (unique.length === 0) return existing;

  // Sort only the small batch; the existing list is already ordered.
  unique.sort(compareTranscripts);

  const last = existing[existing.length - 1];
  if (!last || compareTranscripts(last, unique[0]) <= 0) {
    return [...existing, ...unique];
  }

  // Rare out-of-order batch: merge two ordered lists in one pass.
  const merged: Transcript[] = [];
  let left = 0;
  let right = 0;
  while (left < existing.length && right < unique.length) {
    if (compareTranscripts(existing[left], unique[right]) <= 0) merged.push(existing[left++]);
    else merged.push(unique[right++]);
  }
  while (left < existing.length) merged.push(existing[left++]);
  while (right < unique.length) merged.push(unique[right++]);
  return merged;
}

/** Insert one segment into the ordered list (manual add path, no sequence dedupe). */
export function insertTranscript(existing: Transcript[], transcript: Transcript): Transcript[] {
  const last = existing[existing.length - 1];
  if (!last || compareTranscripts(last, transcript) <= 0) return [...existing, transcript];
  const index = existing.findIndex(item => compareTranscripts(item, transcript) > 0);
  return [...existing.slice(0, index), transcript, ...existing.slice(index)];
}

export function TranscriptProvider({ children }: { children: ReactNode }) {
  const [transcripts, setTranscripts] = useState<Transcript[]>([]);
  const [meetingTitle, setMeetingTitle] = useState('+ New Call');
  const [currentMeetingId, setCurrentMeetingId] = useState<string | null>(null);

  // Recording state context - provides backend-synced state
  const recordingState = useRecordingState();

  // Refs for transcript management
  const transcriptsRef = useRef<Transcript[]>(transcripts);
  const isUserAtBottomRef = useRef<boolean>(true);
  const transcriptContainerRef = useRef<HTMLDivElement>(null);
  const finalFlushRef = useRef<(() => void) | null>(null);

  // Latest recording id for callbacks that outlive a render (background saves)
  const currentMeetingIdRef = useRef<string | null>(currentMeetingId);
  currentMeetingIdRef.current = currentMeetingId;

  // Keep ref updated with current transcripts
  useEffect(() => {
    transcriptsRef.current = transcripts;
  }, [transcripts]);

  // Smart auto-scroll: Track user scroll position
  useEffect(() => {
    const handleScroll = () => {
      const container = transcriptContainerRef.current;
      if (!container) return;

      const { scrollTop, scrollHeight, clientHeight } = container;
      const isAtBottom = scrollTop + clientHeight >= scrollHeight - 10; // 10px tolerance
      isUserAtBottomRef.current = isAtBottom;
    };

    const container = transcriptContainerRef.current;
    if (container) {
      container.addEventListener('scroll', handleScroll);
      return () => container.removeEventListener('scroll', handleScroll);
    }
  }, []);

  // Auto-scroll when transcripts change (only if user is at bottom)
  useEffect(() => {
    // Only auto-scroll if user was at the bottom before new content
    if (isUserAtBottomRef.current && transcriptContainerRef.current) {
      // Wait for Framer Motion animation to complete (150ms) before scrolling
      // This ensures scrollHeight includes the full rendered height of the new transcript
      const scrollTimeout = setTimeout(() => {
        const container = transcriptContainerRef.current;
        if (container) {
          container.scrollTo({
            top: container.scrollHeight,
            behavior: 'smooth'
          });
        }
      }, 150); // Match Framer Motion transition duration

      return () => clearTimeout(scrollTimeout);
    }
  }, [transcripts]);

  // Initialize IndexedDB and listen for recording-started/stopped events
  useEffect(() => {
    let unlistenRecordingStarted: (() => void) | undefined;
    let unlistenRecordingStopped: (() => void) | undefined;

    const setupRecordingListeners = async () => {
      try {
        // Initialize IndexedDB
        await indexedDBService.init();

        // Listen for recording-started event
        unlistenRecordingStarted = await recordingService.onRecordingStarted(async () => {
          try {
            // Generate unique meeting ID
            const meetingId = `meeting-${Date.now()}`;
            setCurrentMeetingId(meetingId);

            // Store in sessionStorage as fallback for markMeetingAsSaved
            sessionStorage.setItem('indexeddb_current_meeting_id', meetingId);

            // Get meeting name
            const meetingName = await recordingService.getRecordingMeetingName();

            // Use a better fallback that matches the backend's naming pattern
            const effectiveTitle = meetingName || `Meeting ${new Date().toISOString().slice(0, 19).replace('T', '_').replace(/:/g, '-')}`;

            // Initialize meeting metadata in IndexedDB
            await indexedDBService.saveMeetingMetadata({
              meetingId,
              title: effectiveTitle,
              startTime: Date.now(),
              lastUpdated: Date.now(),
              transcriptCount: 0,
              savedToSQLite: false,
              folderPath: undefined // Will update shortly
            });

            // Synchronize meeting title to state (fixes tray stop title issue)
            setMeetingTitle(effectiveTitle);

            // Fetch folder path from backend and update metadata
            // This ensures folder path is persisted even if app crashes
            try {
              const { invoke } = await import('@tauri-apps/api/core');
              const folderPath = await invoke<string>('get_meeting_folder_path');
              if (folderPath) {
                const metadata = await indexedDBService.getMeetingMetadata(meetingId);
                if (metadata) {
                  metadata.folderPath = folderPath;
                  await indexedDBService.saveMeetingMetadata(metadata);
                }
              }
            } catch (error) {
              // Non-fatal - will be set on stop if recording completes normally
            }
          } catch (error) {
            console.error('Failed to initialize meeting in IndexedDB:', error);
          }
        });

        // Listen for recording-stopped event
        unlistenRecordingStopped = await recordingService.onRecordingStopped(async (payload) => {
          try {
            if (currentMeetingId) {
              // Update folder path in IndexedDB
              const metadata = await indexedDBService.getMeetingMetadata(currentMeetingId);

              if (metadata && payload.folder_path) {
                metadata.folderPath = payload.folder_path;
                await indexedDBService.saveMeetingMetadata(metadata);
              }
            }
          } catch (error) {
            console.error('Failed to update meeting metadata on stop:', error);
          }
        });
      } catch (error) {
        console.error('Failed to setup recording listeners:', error);
      }
    };

    setupRecordingListeners();

    return () => {
      if (unlistenRecordingStarted) {
        unlistenRecordingStarted();
      }
      if (unlistenRecordingStopped) {
        unlistenRecordingStopped();
      }
    };
  }, [currentMeetingId]);

  // Main transcript buffering logic with sequence_id ordering
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    let transcriptCounter = 0;
    let transcriptBuffer = new Map<number, Transcript>();
    let lastProcessedSequence = 0;
    let processingTimer: NodeJS.Timeout | undefined;

    const processBufferedTranscripts = () => {
      // Drain everything buffered: serial workers deliver in order, and stale or
      // out-of-order segments still have to be shown. mergeTranscripts inserts them
      // in order without re-sorting the whole list on every chunk.
      const drained: Transcript[] = [];

      let nextSequence = lastProcessedSequence + 1;
      while (transcriptBuffer.has(nextSequence)) {
        const buffered = transcriptBuffer.get(nextSequence)!;
        drained.push(buffered);
        transcriptBuffer.delete(nextSequence);
        lastProcessedSequence = nextSequence;
        nextSequence++;
      }

      for (const [sequenceId, transcript] of transcriptBuffer.entries()) {
        drained.push(transcript);
        transcriptBuffer.delete(sequenceId);
      }

      if (drained.length > 0) {
        setTranscripts(prev => mergeTranscripts(prev, drained));
      }
    };

    // Assign final flush function to ref for external access.
    finalFlushRef.current = () => processBufferedTranscripts();

    const setupListener = async () => {
      try {
        unlistenFn = await transcriptService.onTranscriptUpdate((update) => {
          // A stopped recording keeps transcribing its queued audio in the
          // background; those segments are saved with that meeting and must not
          // appear in (or be recovered with) the one recording now.
          if (isFinishedSession(update.session_id)) {
            return;
          }

          // Check for duplicate sequence_id before processing
          if (transcriptBuffer.has(update.sequence_id)) {
            return;
          }

          // Create transcript for buffer with NEW timestamp fields
          const newTranscript: Transcript = {
            id: `${Date.now()}-${transcriptCounter++}`,
            text: update.text,
            timestamp: update.timestamp,
            sequence_id: update.sequence_id,
            chunk_start_time: update.chunk_start_time,
            is_partial: update.is_partial,
            confidence: update.confidence,
            // NEW: Recording-relative timestamps for playback sync
            audio_start_time: update.audio_start_time,
            audio_end_time: update.audio_end_time,
            duration: update.duration,
          };

          // Add to buffer
          transcriptBuffer.set(update.sequence_id, newTranscript);

          // Save to IndexedDB (non-blocking)
          if (currentMeetingId) {
            indexedDBService.saveTranscript(currentMeetingId, update)
              .catch(err => console.warn('IndexedDB save failed:', err));
          }

          // Clear any existing timer and set a new one
          if (processingTimer) {
            clearTimeout(processingTimer);
          }

          // Process buffer with minimal delay for immediate UI updates (serial workers = sequential order)
          processingTimer = setTimeout(processBufferedTranscripts, 10);
        });
      } catch (error) {
        console.error('❌ Failed to setup MAIN transcript listener:', error);
        alert('Failed to setup transcript listener. Check console for details.');
      }
    };

    setupListener();

    return () => {
      if (processingTimer) {
        clearTimeout(processingTimer);
      }
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [currentMeetingId]); // Add currentMeetingId dependency

  // Sync transcript history and meeting name from backend on reload
  // This fixes the issue where reloading during active recording causes state desync
  useEffect(() => {
    const syncFromBackend = async () => {
      // If recording is active and we have no local transcripts, sync from backend
      if (recordingState.isRecording && transcripts.length === 0) {
        try {

          // Fetch transcript history from backend
          const history = await transcriptService.getTranscriptHistory();

          // Convert backend format to frontend Transcript format
          const formattedTranscripts: Transcript[] = history.map((segment) => ({
            id: segment.id,
            text: segment.text,
            timestamp: segment.display_time, // Use display_time for UI
            sequence_id: segment.sequence_id,
            chunk_start_time: segment.audio_start_time,
            is_partial: false, // History segments are always final
            confidence: segment.confidence,
            audio_start_time: segment.audio_start_time,
            audio_end_time: segment.audio_end_time,
            duration: segment.duration,
          }));

          setTranscripts(formattedTranscripts);

          // Fetch meeting name from backend
          const meetingName = await recordingService.getRecordingMeetingName();
          if (meetingName) {
            setMeetingTitle(meetingName);
          }
        } catch (error) {
          console.error('[Reload Sync] Failed to sync from backend:', error);
        }
      }
    };

    syncFromBackend();
  }, [recordingState.isRecording]); // Run when recording state changes

  // Manual transcript update handler (for RecordingControls component)
  const addTranscript = useCallback((update: TranscriptUpdate) => {
    const newTranscript: Transcript = {
      id: update.sequence_id ? update.sequence_id.toString() : Date.now().toString(),
      text: update.text,
      timestamp: update.timestamp,
      sequence_id: update.sequence_id || 0,
      chunk_start_time: update.chunk_start_time,
      is_partial: update.is_partial,
      confidence: update.confidence,
      audio_start_time: update.audio_start_time,
      audio_end_time: update.audio_end_time,
      duration: update.duration,
    };

    setTranscripts(prev => {
      // Preserve the manual path's duplicate rule (same text and timestamp).
      if (prev.some(t => t.text === update.text && t.timestamp === update.timestamp)) {
        return prev;
      }
      return insertTranscript(prev, newTranscript);
    });
  }, []);

  // Copy transcript to clipboard with recording-relative timestamps
  const copyTranscript = useCallback(() => {
    // Format timestamps as recording-relative [MM:SS] instead of wall-clock time
    const formatTime = (seconds: number | undefined): string => {
      if (seconds === undefined) return '[--:--]';
      const totalSecs = Math.floor(seconds);
      const mins = Math.floor(totalSecs / 60);
      const secs = totalSecs % 60;
      return `[${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
    };

    const fullTranscript = transcripts
      .map(t => `${formatTime(t.audio_start_time)} ${t.text}`)
      .join('\n');
    navigator.clipboard.writeText(fullTranscript);

    toast.success(tr('transcript.copied'));
  }, [transcripts]);

  // Force flush buffer (for final transcript processing)
  const flushBuffer = useCallback(() => {
    if (finalFlushRef.current) {
      finalFlushRef.current();
    }
  }, []);

  // Clear transcripts (used when starting new recording)
  const clearTranscripts = useCallback(() => {
    setTranscripts([]);
    // Don't clear currentMeetingId here - it will be set by recording-started event
  }, []);

  // Mark a meeting as saved in IndexedDB. A recording finalized in the
  // background passes its own id: by then the current one may be a newer recording.
  const markMeetingAsSaved = useCallback(async (explicitMeetingId?: string | null) => {
    // Try context state first, fallback to sessionStorage
    const currentId = currentMeetingIdRef.current || sessionStorage.getItem('indexeddb_current_meeting_id');
    const meetingId = explicitMeetingId || currentId;

    if (!meetingId) {
      console.error('[IndexedDB] ❌ Cannot mark meeting as saved: No meeting ID available!');
      console.error('[IndexedDB] currentMeetingId:', currentMeetingId);
      console.error('[IndexedDB] sessionStorage:', sessionStorage.getItem('indexeddb_current_meeting_id'));
      return;
    }

    try {
      await indexedDBService.markMeetingSaved(meetingId);

      // Clear both sources, unless they already point at a newer recording
      if (meetingId === currentId) {
        setCurrentMeetingId(null);
        sessionStorage.removeItem('indexeddb_current_meeting_id');
      }
    } catch (error) {
      console.error('[IndexedDB] ❌ Failed to mark meeting as saved:', error);
    }
  }, [currentMeetingId]);

  const value: TranscriptContextType = {
    transcripts,
    transcriptsRef,
    addTranscript,
    copyTranscript,
    flushBuffer,
    transcriptContainerRef,
    meetingTitle,
    setMeetingTitle,
    clearTranscripts,
    currentMeetingId,
    markMeetingAsSaved,
  };

  return (
    <TranscriptContext.Provider value={value}>
      {children}
    </TranscriptContext.Provider>
  );
}

export function useTranscripts() {
  const context = useContext(TranscriptContext);
  if (context === undefined) {
    throw new Error('useTranscripts must be used within a TranscriptProvider');
  }
  return context;
}
