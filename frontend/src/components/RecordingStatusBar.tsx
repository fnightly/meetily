'use client';

import { motion } from 'framer-motion';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useEffect, useState } from 'react';
import { useI18n } from '@/contexts/I18nContext';

interface RecordingStatusBarProps {
  isPaused?: boolean;
}

export const RecordingStatusBar: React.FC<RecordingStatusBarProps> = ({ isPaused = false }) => {
  // Get recording duration from backend-synced context (in seconds)
  // Backend polls every 500ms, providing smooth updates
  const { activeDuration } = useRecordingState();
  const { t } = useI18n();

  // Display state synced from backend
  const [displaySeconds, setDisplaySeconds] = useState(0);

  // Sync with backend duration when it changes (handles refresh/navigation)
  useEffect(() => {
    if (activeDuration !== null) {
      // Round to nearest second to avoid decimal issues
      setDisplaySeconds(Math.floor(activeDuration));
    }
  }, [activeDuration]);

  const formatDuration = (seconds: number): string => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  };

  return (
    <motion.span
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.2 }}
      className={`chip ${isPaused ? 'bg-amber-50 text-amber-700' : 'bg-red-50 text-red-600'}`}
      role="status"
    >
      <span className={`h-1.5 w-1.5 rounded-full ${isPaused ? 'bg-amber-500' : 'bg-red-500 animate-pulse'}`} />
      {isPaused ? t('live.paused') : t('live.recording')}
      <span className="tabular-nums font-semibold">{formatDuration(displaySeconds)}</span>
    </motion.span>
  );
};
