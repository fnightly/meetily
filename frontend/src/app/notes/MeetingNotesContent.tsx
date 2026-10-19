"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/contexts/I18nContext";
import { MeetingNotesEditor } from "./MeetingNotesEditor";

/**
 * The standalone /notes route.
 *
 * The editor itself lives in `MeetingNotesEditor` and is shared with the 笔记
 * tab in the meeting side pane. This file only owns the route chrome (back
 * button, page padding) and the missing-id case; saving, dirty tracking and
 * image handling stay in the shared component.
 *
 * The meeting id arrives as a `?id=` query parameter rather than a path segment:
 * the app builds with `output: 'export'`, and a dynamic `[id]` route would
 * require `generateStaticParams()`, which cannot enumerate meetings that only
 * exist at runtime. `/meeting-details` uses the same approach.
 */
export function MeetingNotesContent() {
  const searchParams = useSearchParams();
  const router = useRouter();
  const { t } = useI18n();
  const meetingId = searchParams.get("id") ?? "";

  // No meeting selected: what a user sees when opening /notes directly, or when
  // the query string is missing. Checked before rendering the editor so an
  // absent id never renders as a spinner that cannot resolve.
  if (!meetingId) {
    return (
      <div className="mx-auto max-w-4xl p-8">
        <h1 className="mb-2 text-2xl font-semibold">{t("notes.title")}</h1>
        <p className="mb-4 text-sm text-gray-600">{t("notes.noMeeting")}</p>
        <Button variant="outline" onClick={() => router.push("/")}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          {t("common.back")}
        </Button>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-4xl p-8">
      <Button
        variant="ghost"
        size="sm"
        className="-ml-2 mb-3"
        onClick={() => router.back()}
      >
        <ArrowLeft className="mr-2 h-4 w-4" />
        {t("common.back")}
      </Button>
      <MeetingNotesEditor meetingId={meetingId} />
    </div>
  );
}
