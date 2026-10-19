'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import {
  configureBackgroundTranscription,
  ensureBackgroundTranscriptionListeners,
} from '@/lib/background-transcription';

/**
 * Connects the background transcription queue to the app: its "View" buttons
 * navigate with the app router, and finished jobs reload the meeting list.
 * Mounted once for the app's lifetime so no result event is missed.
 */
export function BackgroundTranscriptionHost() {
  const router = useRouter();
  const { refetchMeetings } = useSidebar();

  useEffect(() => {
    void ensureBackgroundTranscriptionListeners();
  }, []);

  useEffect(() => {
    configureBackgroundTranscription({
      navigate: (href) => router.push(href),
      refreshMeetings: refetchMeetings,
    });
    return () => configureBackgroundTranscription(null);
  }, [router, refetchMeetings]);

  return null;
}
