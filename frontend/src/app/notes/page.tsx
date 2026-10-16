"use client";

import { Suspense } from "react";
import { Loader2 } from "lucide-react";
import { MeetingNotesContent } from "./MeetingNotesContent";

/**
 * Meeting notes route.
 *
 * The meeting id is a `?id=` query parameter, not a `[id]` path segment: the app
 * builds with `output: 'export'`, so a dynamic segment would need
 * `generateStaticParams()` to enumerate every possible id — impossible here,
 * because meetings and their notes only exist at runtime. `/meeting-details`
 * takes the same approach.
 *
 * `useSearchParams` must be inside a `Suspense` boundary or the static export
 * build fails while collecting page data.
 */
export default function NotesPage() {
  return (
    <Suspense
      fallback={
        <div className="p-8 flex items-center gap-2 text-gray-600">
          <Loader2 className="w-4 h-4 animate-spin" />
          Loading note…
        </div>
      }
    >
      <MeetingNotesContent />
    </Suspense>
  );
}
