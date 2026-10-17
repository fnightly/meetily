'use client';

import { useCallback, useRef, useReducer, startTransition, useEffect, useState, memo } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { motion, AnimatePresence } from "framer-motion";
import { TranscriptSegmentData } from "@/types";
import { useI18n } from "@/contexts/I18nContext";
import { displaySpeaker, useSpeakerNames } from "@/lib/speaker-names";

export interface VirtualizedTranscriptViewProps {
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
    /** Whether recording is in progress */
    isRecording?: boolean;
    /** Whether recording is paused */
    isPaused?: boolean;
    /** Whether processing/finalizing transcription */
    isProcessing?: boolean;
    /** Whether stopping */
    isStopping?: boolean;
    /** Enable streaming effect for latest segment */
    enableStreaming?: boolean;
    /** Show confidence indicators */
    showConfidence?: boolean;
    /** Completely disable auto-scroll behavior (for meeting details page) */
    disableAutoScroll?: boolean;

    // Pagination props (infinite scroll)
    hasMore?: boolean;
    isLoadingMore?: boolean;
    totalCount?: number;
    loadedCount?: number;
    onLoadMore?: () => void;

    /** Recording-relative second to scroll to once loaded (search / Q&A deep links). */
    focusTime?: number | null;
    /** Text to mark in every row (the search query that led here). */
    highlightQuery?: string | null;
}

// Threshold for enabling virtualization (below this, use simple rendering)
const VIRTUALIZATION_THRESHOLD = 10;

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined) return '[--:--]';

    const totalSeconds = Math.floor(seconds);
    const minutes = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;

    return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

// Helper function to remove filler words and repetitions
function cleanStopWords(text: string): string {
    const stopWords = ['uh', 'um', 'er', 'ah', 'hmm', 'hm', 'eh', 'oh'];

    let cleanedText = text;
    stopWords.forEach(word => {
        const pattern = new RegExp(`\\b${word}\\b[,\\s]*`, 'gi');
        cleanedText = cleanedText.replace(pattern, ' ');
    });

    return cleanedText.replace(/\s+/g, ' ').trim();
}

function escapeRegExp(value: string): string {
    return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Wrap case-insensitive matches of `query` in <mark>. */
function highlightText(text: string, query?: string | null) {
    const q = query?.trim();
    if (!q) return text;
    const parts = text.split(new RegExp(`(${escapeRegExp(q)})`, 'gi'));
    if (parts.length === 1) return text;
    return parts.map((part, i) =>
        i % 2 === 1
            ? <mark key={i} className="rounded-sm bg-amber-200/80 px-0.5 text-inherit">{part}</mark>
            : part
    );
}

// Memoized transcript segment component
const TranscriptSegment = memo(function TranscriptSegment({
    id,
    timestamp,
    text,
    confidence,
    speaker,
    showSpeaker,
    isStreaming,
    showConfidence,
    highlightQuery,
    flash = false,
}: {
    id: string;
    timestamp: number;
    text: string;
    confidence?: number;
    /** Speaker label, or null when the meeting has not been separated. */
    speaker?: string | null;
    /**
     * Whether to render the speaker label on this row.
     *
     * Set only on the first row of a run so consecutive turns by one person read as a
     * block rather than repeating the name on every line. Computed by the parent because
     * a virtualized row must not depend on its neighbours to decide its own height.
     */
    showSpeaker: boolean;
    isStreaming: boolean;
    showConfidence: boolean;
    highlightQuery?: string | null;
    /** Briefly highlight this row (it is the target of a deep link). */
    flash?: boolean;
}) {
    const displayText = cleanStopWords(text) || (text.trim() === '' ? '[Silence]' : text);

    // Called here rather than passed down so the memo boundary still holds: only the
    // props that affect this row's markup cause a re-render.
    const { t } = useI18n();
    const { names, requestRename } = useSpeakerNames();
    const speakerLabelText = speaker ? displaySpeaker(speaker, t, names) : null;
    const isNamed = !!(speaker && names[speaker]?.trim());

    return (
        <div
            id={`segment-${id}`}
            className={`${showSpeaker ? 'mb-3 mt-4' : 'mb-3'} rounded-lg transition-colors duration-700 ${flash ? 'bg-amber-50 ring-1 ring-amber-200' : ''}`}
        >
            {showSpeaker && speakerLabelText && (
                <div className="mb-1 ml-[58px]">
                    {requestRename && speaker ? (
                        <button
                            type="button"
                            onClick={() => requestRename(speaker)}
                            title={t('speakers.clickToRename')}
                            className={`inline-block rounded-full px-2 py-0.5 text-xs font-medium transition-colors ${isNamed ? 'bg-blue-50 text-blue-700 hover:bg-blue-100' : 'bg-gray-100 text-gray-700 hover:bg-gray-200'}`}
                        >
                            {speakerLabelText}
                        </button>
                    ) : (
                        <span className="inline-block rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-700">
                            {speakerLabelText}
                        </span>
                    )}
                </div>
            )}
            <div className="flex items-start gap-2">
                <Tooltip>
                    <TooltipTrigger>
                        <span className="text-xs text-gray-400 mt-1 flex-shrink-0 min-w-[50px]">
                            {formatRecordingTime(timestamp)}
                        </span>
                    </TooltipTrigger>
                    <TooltipContent>
                        {confidence !== undefined && showConfidence && (
                            <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                        )}
                    </TooltipContent>
                </Tooltip>
                <div className="flex-1">
                    {isStreaming ? (
                        <div className="bg-gray-100 border border-gray-200 rounded-lg px-3 py-2">
                            <p className="text-base text-gray-800 leading-relaxed">{displayText}</p>
                        </div>
                    ) : (
                        <p className="text-base text-gray-800 leading-relaxed">{highlightText(displayText, highlightQuery)}</p>
                    )}
                </div>
            </div>
        </div>
    );
});

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    segments,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
    focusTime = null,
    highlightQuery = null,
}) => {
    const { t } = useI18n();
    const [flashId, setFlashId] = useState<string | null>(null);
    const focusedForRef = useRef<number | null>(null);
    // Create scroll ref first - shared between virtualizer and auto-scroll hook
    const scrollRef = useRef<HTMLDivElement>(null);
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const virtualizer = useVirtualizer({
        count: segments.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 60, // Estimated height per segment
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    // Custom hook for auto-scrolling (supports both virtualized and non-virtualized)
    useAutoScroll({
        scrollRef,
        segments,
        isRecording,
        isPaused,
        virtualizer,
        virtualizationThreshold: VIRTUALIZATION_THRESHOLD,
        disableAutoScroll,
    });

    // Deep link: load pages until the target second is present, then centre and flash it.
    useEffect(() => {
        if (focusTime == null || !Number.isFinite(focusTime) || focusedForRef.current === focusTime) return;
        if (segments.length === 0) return;
        const last = segments[segments.length - 1];
        if ((last.timestamp ?? 0) < focusTime && hasMore) {
            if (!isLoadingMore) onLoadMore?.();
            return;
        }
        let index = 0;
        for (let i = 0; i < segments.length; i++) {
            if ((segments[i].timestamp ?? 0) <= focusTime + 0.5) index = i;
            else break;
        }
        focusedForRef.current = focusTime;
        const target = segments[index];
        const scroll = () => {
            if (segments.length >= VIRTUALIZATION_THRESHOLD) {
                virtualizer.scrollToIndex(index, { align: 'center' });
            } else {
                document.getElementById(`segment-${target.id}`)?.scrollIntoView({ block: 'center' });
            }
        };
        // Twice: the first pass measures rows near the target, the second lands on it.
        requestAnimationFrame(scroll);
        const settle = window.setTimeout(scroll, 180);
        setFlashId(target.id);
        const clear = window.setTimeout(() => setFlashId(null), 2600);
        return () => { window.clearTimeout(settle); window.clearTimeout(clear); };
    }, [focusTime, segments, hasMore, isLoadingMore, onLoadMore, virtualizer]);

    // Streaming text effect hook (typewriter animation for new transcripts)
    const { streamingSegmentId, getDisplayText } = useTranscriptStreaming(
        segments,
        isRecording,
        enableStreaming
    );

    // Infinite scroll: IntersectionObserver to trigger loading more
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording || segments.length === 0) {
            return;
        }

        const triggerElement = loadMoreTriggerRef.current;
        if (!triggerElement) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0].isIntersecting && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
            },
            {
                root: null,
                rootMargin: '100px',
                threshold: 0,
            }
        );

        observer.observe(triggerElement);

        return () => observer.disconnect();
    }, [hasMore, isLoadingMore, onLoadMore, isRecording, segments.length]);

    // Scroll-based fallback for fast scrolling
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording) return;

        const scrollElement = scrollRef.current;
        if (!scrollElement) return;

        let ticking = false;

        const handleScroll = () => {
            if (ticking || isLoadingMore || !hasMore) return;

            ticking = true;
            requestAnimationFrame(() => {
                const { scrollTop, scrollHeight, clientHeight } = scrollElement;
                const scrollBottom = scrollHeight - scrollTop - clientHeight;

                // Trigger load when within 200px of bottom
                if (scrollBottom < 200 && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
                ticking = false;
            });
        };

        scrollElement.addEventListener('scroll', handleScroll, { passive: true });
        return () => scrollElement.removeEventListener('scroll', handleScroll);
    }, [onLoadMore, hasMore, isLoadingMore, isRecording]);

    // Use simple rendering for small lists, virtualization for large lists
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;

    return (
        <div ref={scrollRef} className="flex flex-col h-full overflow-y-auto px-4 py-2">
            {/* Recording Status Bar - Sticky at top, always visible when recording */}

            {/* Content - add padding when recording to prevent overlap */}
            <div className={isRecording ? 'pt-2' : ''}>
            {segments.length === 0 ? (
                // Empty state
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    className="text-center text-slate-500 mt-16"
                >
                    {isRecording ? (
                        <>
                            <div className="flex items-end justify-center gap-1 h-8 mb-4" aria-hidden>
                                {[0, 1, 2, 3, 4].map((i) => (
                                    <span
                                        key={i}
                                        className={`w-1.5 rounded-full ${isPaused ? 'bg-amber-300 h-2' : 'bg-blue-400 animate-pulse'}`}
                                        style={isPaused ? undefined : { height: `${[40, 75, 100, 65, 35][i]}%`, animationDelay: `${i * 120}ms` }}
                                    />
                                ))}
                            </div>
                            <p className="text-sm font-medium text-slate-700">
                                {isPaused ? t('live.pausedTitle') : t('live.listeningTitle')}
                            </p>
                            <p className="text-xs mt-1 text-slate-400">
                                {isPaused ? t('live.pausedHint') : t('live.listeningHint')}
                            </p>
                        </>
                    ) : (
                        <>
                            <p className="text-sm font-medium text-slate-600">{t('live.emptyTitle')}</p>
                            <p className="text-xs mt-1 text-slate-400">{t('live.emptyHint')}</p>
                        </>
                    )}
                </motion.div>
            ) : useVirtualization ? (
                // Virtualized rendering for large lists
                <>
                    <div
                        style={{
                            height: virtualizer.getTotalSize(),
                            width: "100%",
                            position: "relative",
                        }}
                    >
                        {virtualizer.getVirtualItems().map((virtualRow) => {
                            const segment = segments[virtualRow.index];
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <div
                                    key={segment.id}
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        // Show the label only when the speaker changes, so a run
                                        // of one person's speech reads as a block.
                                        showSpeaker={
                                            !!segment.speaker &&
                                            segment.speaker !== segments[virtualRow.index - 1]?.speaker
                                        }
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        highlightQuery={highlightQuery}
                                        flash={flashId === segment.id}
                                    />
                                </div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger and loading indicator */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">{t('transcript.loadingMore')}</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    {t('transcript.showingOf', { loaded: loadedCount, total: totalCount })}
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-gray-500"
                        >
                            <div className="w-2 h-2 bg-blue-500 rounded-full animate-pulse"></div>
                            <span className="text-sm">{t('live.listening')}</span>
                        </motion.div>
                    )}
                </>
            ) : (
                // Simple rendering for small lists (better animations)
                <>
                    <div className="space-y-1">
                        {segments.map((segment, index) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        // Same rule as the virtualized path: label only when
                                        // the speaker changes.
                                        showSpeaker={
                                            !!segment.speaker &&
                                            segment.speaker !== segments[index - 1]?.speaker
                                        }
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        highlightQuery={highlightQuery}
                                        flash={flashId === segment.id}
                                    />
                                </motion.div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger (for small lists that grow) */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">{t('transcript.loadingMore')}</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    {t('transcript.showingOf', { loaded: loadedCount, total: totalCount })}
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-gray-500"
                        >
                            <div className="w-2 h-2 bg-blue-500 rounded-full animate-pulse"></div>
                            <span className="text-sm">{t('live.listening')}</span>
                        </motion.div>
                    )}
                </>
            )}
            </div>
        </div>
    );
};
