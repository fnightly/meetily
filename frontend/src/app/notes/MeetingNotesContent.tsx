"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { ArrowLeft, ImagePlus, Loader2, Plus, Save, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { tr } from '@/lib/i18n';

/**
 * Meeting notes editor.
 *
 * Notes are persisted through the Tauri commands in
 * `src-tauri/src/database/notes_commands.rs`; the page previously rendered
 * hardcoded sample data and never touched the backend.
 *
 * Saving is explicit (not on every keystroke) because each save writes the whole
 * document plus a derived markdown rendering. A dirty indicator plus a
 * before-unload guard keeps that from losing work.
 *
 * The meeting id arrives as a `?id=` query parameter rather than a path segment:
 * the app builds with `output: 'export'`, and a dynamic `[id]` route would
 * require `generateStaticParams()`, which cannot enumerate meetings that only
 * exist at runtime. `/meeting-details` uses the same approach.
 */

type NoteBlock =
  | { type: "text"; text: string }
  | { type: "bullet"; text: string }
  | { type: "heading"; text: string; level: number }
  | { type: "image"; path: string; alt: string };

interface MeetingNote {
  meeting_id: string;
  blocks: NoteBlock[];
  created_at: string;
  updated_at: string;
  exists: boolean;
}

interface SavedNoteImage {
  path: string;
  file_name: string;
  bytes: number;
}

interface MeetingMetadata {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
}

/**
 * Render a note image.
 *
 * Images come back as data URLs from `api_read_note_image` rather than through
 * Tauri's asset protocol: the notes folder is user-configurable and the asset
 * scope only covers `$APPDATA/**`, so a note stored elsewhere would render blank.
 */
function NoteImage({ path, alt }: { path: string; alt: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    setFailed(false);

    invoke<string>("api_read_note_image", { path })
      .then((dataUrl) => {
        if (!cancelled) setSrc(dataUrl);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });

    return () => {
      cancelled = true;
    };
  }, [path]);

  if (failed) {
    return (
      <div className="text-xs text-amber-700 bg-amber-50 border border-amber-200 rounded p-2">
        Image could not be loaded (the file may have been moved or deleted).
      </div>
    );
  }

  if (!src) {
    return (
      <div className="flex items-center gap-2 text-xs text-gray-500">
        <Loader2 className="w-3 h-3 animate-spin" />
        Loading image…
      </div>
    );
  }

  return <img src={src} alt={alt || "Note image"} className="max-h-80 rounded" />;
}

export function MeetingNotesContent() {
  const searchParams = useSearchParams();
  const router = useRouter();
  const meetingId = searchParams.get("id") ?? "";

  const [blocks, setBlocks] = useState<NoteBlock[]>([]);
  const [metadata, setMetadata] = useState<MeetingMetadata | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Warn before leaving with unsaved edits.
  useEffect(() => {
    if (!dirty) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [dirty]);

  useEffect(() => {
    if (!meetingId) {
      // Reachable during static export, where no query string exists yet.
      setLoading(false);
      return;
    }

    let cancelled = false;

    const load = async () => {
      setLoading(true);
      setError(null);
      try {
        const [note, meta] = await Promise.all([
          invoke<MeetingNote>("api_get_meeting_note", { meetingId }),
          // Metadata is best-effort: a note can still be edited without it.
          invoke<MeetingMetadata>("api_get_meeting_metadata", { meetingId }).catch(() => null),
        ]);
        if (cancelled) return;
        setBlocks(note.blocks ?? []);
        setMetadata(meta);
        setDirty(false);
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    void load();
    return () => {
      cancelled = true;
    };
  }, [meetingId]);

  const updateBlock = useCallback((index: number, patch: Partial<NoteBlock>) => {
    setBlocks((prev) =>
      prev.map((b, i) => (i === index ? ({ ...b, ...patch } as NoteBlock) : b)),
    );
    setDirty(true);
  }, []);

  const addBlock = useCallback((type: NoteBlock["type"]) => {
    setBlocks((prev) => {
      const next: NoteBlock =
        type === "heading"
          ? { type: "heading", text: "", level: 2 }
          : type === "bullet"
            ? { type: "bullet", text: "" }
            : { type: "text", text: "" };
      return [...prev, next];
    });
    setDirty(true);
  }, []);

  const removeBlock = useCallback((index: number) => {
    setBlocks((prev) => prev.filter((_, i) => i !== index));
    setDirty(true);
  }, []);

  const handleSave = useCallback(async () => {
    if (!meetingId) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await invoke<MeetingNote>("api_save_meeting_note", {
        meetingId,
        note: { blocks },
      });
      setBlocks(saved.blocks ?? []);
      setDirty(false);
      toast.success(tr('notes.saved'));
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setError(message);
      toast.error(tr('notes.saveFailed'), { description: message });
    } finally {
      setSaving(false);
    }
  }, [meetingId, blocks]);

  /** Upload image bytes; the backend writes them to the notes folder. */
  const handleImageFiles = useCallback(
    async (files: FileList | null) => {
      if (!files || files.length === 0) return;
      setUploading(true);
      setError(null);
      try {
        const added: NoteBlock[] = [];

        for (const file of Array.from(files)) {
          if (!file.type.startsWith("image/")) {
            toast.error(`Skipped "${file.name}"`, { description: "Only image files can be added." });
            continue;
          }

          const buffer = await file.arrayBuffer();
          const saved = await invoke<SavedNoteImage>("api_save_note_image", {
            fileName: file.name,
            bytes: Array.from(new Uint8Array(buffer)),
          });

          added.push({ type: "image", path: saved.path, alt: file.name });
        }

        if (added.length > 0) {
          setBlocks((prev) => [...prev, ...added]);
          setDirty(true);
          toast.success(`Added ${added.length} image${added.length === 1 ? "" : "s"}`, {
            description: "Save the note to keep them.",
          });
        }
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        setError(message);
        toast.error(tr('notes.imageFailed'), { description: message });
      } finally {
        setUploading(false);
        if (fileInputRef.current) fileInputRef.current.value = "";
      }
    },
    [],
  );

  const handleDeleteImage = useCallback(
    async (index: number) => {
      const block = blocks[index];
      if (!block || block.type !== "image") return;
      try {
        await invoke<boolean>("api_delete_note_image", { path: block.path });
      } catch (e) {
        // A missing file should not block removing the reference.
        console.warn("Failed to delete note image file:", e);
      }
      removeBlock(index);
    },
    [blocks, removeBlock],
  );

  const title = useMemo(
    () => metadata?.title?.trim() || "Meeting Notes",
    [metadata],
  );

  // No meeting selected: what a user sees when opening /notes directly, or when
  // the query string is missing. Checked before the loading state so an absent
  // id never renders as a spinner that cannot resolve.
  //
  // (The HTML baked into the static export is the Suspense fallback in
  // `page.tsx`, not this branch — React does not prerender content behind a
  // Suspense boundary. That fallback is a spinner, matching what /meeting-details
  // does, and the client takes over as soon as it hydrates.)
  if (!meetingId) {
    return (
      <div className="p-8 max-w-4xl mx-auto">
        <h1 className="text-2xl font-semibold mb-2">Meeting Notes</h1>
        <p className="text-sm text-gray-600 mb-4">
          Select a meeting from the sidebar and choose <strong>Open notes</strong> to write notes
          for it.
        </p>
        <Button variant="outline" onClick={() => router.push("/")}>
          <ArrowLeft className="w-4 h-4 mr-2" />
          Back to meetings
        </Button>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="p-8 flex items-center gap-2 text-gray-600">
        <Loader2 className="w-4 h-4 animate-spin" />
        Loading note…
      </div>
    );
  }

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <div className="mb-6 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <Button variant="ghost" size="sm" className="-ml-2 mb-2" onClick={() => router.back()}>
            <ArrowLeft className="w-4 h-4 mr-2" />
            Back
          </Button>
          <h1 className="text-3xl font-bold truncate">{title}</h1>
          <p className="text-xs text-gray-500 font-mono mt-1">{meetingId}</p>
        </div>

        <div className="flex items-center gap-2 shrink-0">
          {uploading && <Loader2 className="w-4 h-4 animate-spin text-gray-500" />}
          <Button
            variant="outline"
            size="sm"
            onClick={() => fileInputRef.current?.click()}
            disabled={uploading}
          >
            <ImagePlus className="w-4 h-4 mr-2" />
            Add image
          </Button>
          <Button size="sm" onClick={handleSave} disabled={saving || !dirty}>
            {saving ? (
              <Loader2 className="w-4 h-4 mr-2 animate-spin" />
            ) : (
              <Save className="w-4 h-4 mr-2" />
            )}
            {dirty ? "Save" : "Saved"}
          </Button>
        </div>
      </div>

      <input
        ref={fileInputRef}
        type="file"
        accept="image/*"
        multiple
        className="hidden"
        onChange={(e) => handleImageFiles(e.target.files)}
      />

      {error && (
        <div role="alert" className="mb-4 p-3 bg-red-50 border border-red-200 rounded-md">
          <p className="text-xs text-red-800">{error}</p>
        </div>
      )}

      {blocks.length === 0 && (
        <p className="text-sm text-gray-600 mb-4">
          No notes yet. Add a heading, some text, or an image to start.
        </p>
      )}

      <div className="space-y-3">
        {blocks.map((block, index) => (
          <div key={index} className="group flex gap-2 items-start">
            <div className="flex-1 min-w-0">
              {block.type === "heading" && (
                <div className="flex gap-2">
                  <select
                    aria-label={`Heading level for block ${index + 1}`}
                    className="h-9 rounded-md border border-gray-200 bg-white px-2 text-sm"
                    value={block.level}
                    onChange={(e) =>
                      updateBlock(index, { level: Number(e.target.value) } as Partial<NoteBlock>)
                    }
                  >
                    {[1, 2, 3].map((l) => (
                      <option key={l} value={l}>
                        H{l}
                      </option>
                    ))}
                  </select>
                  <Input
                    value={block.text}
                    placeholder="Heading"
                    aria-label={`Heading text for block ${index + 1}`}
                    className="font-semibold"
                    onChange={(e) => updateBlock(index, { text: e.target.value } as Partial<NoteBlock>)}
                  />
                </div>
              )}

              {block.type === "text" && (
                <Textarea
                  value={block.text}
                  placeholder="Write something…"
                  aria-label={`Note text for block ${index + 1}`}
                  className="min-h-[70px]"
                  onChange={(e) => updateBlock(index, { text: e.target.value } as Partial<NoteBlock>)}
                />
              )}

              {block.type === "bullet" && (
                <div className="flex items-center gap-2">
                  <span aria-hidden="true" className="text-gray-400">
                    •
                  </span>
                  <Input
                    value={block.text}
                    placeholder="List item"
                    aria-label={`Bullet text for block ${index + 1}`}
                    onChange={(e) =>
                      updateBlock(index, { text: e.target.value } as Partial<NoteBlock>)
                    }
                  />
                </div>
              )}

              {block.type === "image" && (
                <figure className="border rounded-lg p-2 bg-gray-50">
                  <NoteImage path={block.path} alt={block.alt} />
                  <figcaption className="mt-2 flex items-center gap-2">
                    <Input
                      value={block.alt}
                      placeholder="Caption"
                      aria-label={`Caption for image ${index + 1}`}
                      onChange={(e) =>
                        updateBlock(index, { alt: e.target.value } as Partial<NoteBlock>)
                      }
                    />
                  </figcaption>
                </figure>
              )}
            </div>

            <Button
              variant="ghost"
              size="sm"
              aria-label={`Remove block ${index + 1}`}
              title="Remove"
              onClick={() =>
                block.type === "image" ? handleDeleteImage(index) : removeBlock(index)
              }
            >
              {block.type === "image" ? (
                <X className="w-4 h-4" />
              ) : (
                <Trash2 className="w-4 h-4" />
              )}
            </Button>
          </div>
        ))}
      </div>

      <div className="flex flex-wrap gap-2 mt-6 pt-4 border-t">
        <Button variant="outline" size="sm" onClick={() => addBlock("heading")}>
          <Plus className="w-4 h-4 mr-2" />
          Heading
        </Button>
        <Button variant="outline" size="sm" onClick={() => addBlock("text")}>
          <Plus className="w-4 h-4 mr-2" />
          Text
        </Button>
        <Button variant="outline" size="sm" onClick={() => addBlock("bullet")}>
          <Plus className="w-4 h-4 mr-2" />
          Bullet
        </Button>
        <Button variant="outline" size="sm" onClick={() => fileInputRef.current?.click()}>
          <ImagePlus className="w-4 h-4 mr-2" />
          Image
        </Button>
      </div>
    </div>
  );
}
