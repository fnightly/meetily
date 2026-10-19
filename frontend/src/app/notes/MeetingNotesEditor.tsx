"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { ImagePlus, Loader2, Plus, Save, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/contexts/I18nContext";

/**
 * Meeting notes editor.
 *
 * Notes are persisted through the Tauri commands in
 * `src-tauri/src/database/notes_commands.rs`. Saving is explicit (not on every
 * keystroke) because each save writes the whole document plus a derived
 * markdown rendering; a dirty indicator and a before-unload guard keep that from
 * losing work.
 *
 * The same component backs both the standalone `/notes` route and the 笔记 tab
 * in the meeting side pane. `embedded` only changes the surrounding chrome —
 * saving, dirty tracking and image upload are identical in both placements, so
 * there is no second save path to keep in sync.
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
  const { t } = useI18n();
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
      <div className="rounded border border-amber-200 bg-amber-50 p-2 text-xs text-amber-700">
        {t("notes.imageLoadFailed")}
      </div>
    );
  }

  if (!src) {
    return (
      <div className="flex items-center gap-2 text-xs text-gray-500">
        <Loader2 className="h-3 w-3 animate-spin" />
        {t("notes.imageLoading")}
      </div>
    );
  }

  return <img src={src} alt={alt || t("notes.imageAlt")} className="max-h-80 rounded" />;
}

export interface MeetingNotesEditorProps {
  meetingId: string;
  /**
   * Render for the meeting side pane: no page title or route chrome, a compact
   * header, and an internally scrolling body.
   */
  embedded?: boolean;
  /** Notifies the parent when unsaved edits appear or disappear. */
  onDirtyChange?: (dirty: boolean) => void;
  className?: string;
}

export function MeetingNotesEditor({
  meetingId,
  embedded = false,
  onDirtyChange,
  className,
}: MeetingNotesEditorProps) {
  const { t } = useI18n();

  const [blocks, setBlocks] = useState<NoteBlock[]>([]);
  const [metadata, setMetadata] = useState<MeetingMetadata | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Let the embedding pane show an unsaved dot next to the tab label.
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  // Warn before leaving with unsaved edits. Works the same whether the editor
  // is the standalone page or a tab in the meeting pane.
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
      toast.success(t("notes.saved"));
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setError(message);
      toast.error(t("notes.saveFailed"), { description: message });
    } finally {
      setSaving(false);
    }
  }, [meetingId, blocks, t]);

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
            toast.error(t("notes.skipImage", { name: file.name }), {
              description: t("notes.imageOnly"),
            });
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
          toast.success(
            t(added.length === 1 ? "notes.imageAdded" : "notes.imagesAdded", { count: added.length }),
            { description: t("notes.imagesAddedHint") },
          );
        }
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        setError(message);
        toast.error(t("notes.imageFailed"), { description: message });
      } finally {
        setUploading(false);
        if (fileInputRef.current) fileInputRef.current.value = "";
      }
    },
    [t],
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
    () => metadata?.title?.trim() || t("notes.title"),
    [metadata, t],
  );

  const actions = (
    <div className="flex shrink-0 items-center gap-2">
      {uploading && <Loader2 className="h-4 w-4 animate-spin text-gray-500" />}
      <Button
        variant="outline"
        size="sm"
        onClick={() => fileInputRef.current?.click()}
        disabled={uploading}
      >
        <ImagePlus className="mr-2 h-4 w-4" />
        {t("notes.addImage")}
      </Button>
      <Button size="sm" onClick={() => void handleSave()} disabled={saving || !dirty}>
        {saving ? (
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        ) : (
          <Save className="mr-2 h-4 w-4" />
        )}
        {dirty ? t("common.save") : t("notes.savedState")}
      </Button>
    </div>
  );

  return (
    <div
      className={`${embedded ? "flex h-full min-h-0 flex-col" : "w-full"} ${className ?? ""}`}
    >
      <input
        ref={fileInputRef}
        type="file"
        accept="image/*"
        multiple
        className="hidden"
        onChange={(e) => void handleImageFiles(e.target.files)}
      />

      {embedded ? (
        <div className="flex shrink-0 flex-wrap items-start justify-between gap-2 border-b border-slate-100 px-4 py-3">
          <p className="max-w-prose text-xs leading-relaxed text-slate-500">
            {t("notes.embeddedHint")}
          </p>
          {actions}
        </div>
      ) : (
        <div className="mb-6 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="truncate text-3xl font-bold">{title}</h1>
            <p className="mt-1 font-mono text-xs text-gray-500">{meetingId}</p>
          </div>
          {actions}
        </div>
      )}

      <div className={embedded ? "min-h-0 flex-1 overflow-y-auto px-4 py-3" : undefined}>
        {loading ? (
          <div className="flex items-center gap-2 p-2 text-sm text-gray-600">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("common.loading")}
          </div>
        ) : (
          <>
            {error && (
              <div role="alert" className="mb-4 rounded-md border border-red-200 bg-red-50 p-3">
                <p className="text-xs text-red-800">{error}</p>
              </div>
            )}

            {blocks.length === 0 && (
              <div className="mb-4 rounded-lg border border-dashed border-slate-200 bg-slate-50/60 p-4">
                <p className="text-sm font-medium text-slate-700">{t("notes.emptyTitle")}</p>
                <p className="mt-1 text-xs leading-relaxed text-slate-500">{t("notes.emptyBody")}</p>
              </div>
            )}

            <div className="space-y-3">
              {blocks.map((block, index) => (
                <div key={index} className="group flex items-start gap-2">
                  <div className="min-w-0 flex-1">
                    {block.type === "heading" && (
                      <div className="flex gap-2">
                        <select
                          aria-label={t("notes.block.headingLevel", { index: index + 1 })}
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
                          placeholder={t("notes.editor.heading")}
                          aria-label={t("notes.block.headingText", { index: index + 1 })}
                          className="font-semibold"
                          onChange={(e) =>
                            updateBlock(index, { text: e.target.value } as Partial<NoteBlock>)
                          }
                        />
                      </div>
                    )}

                    {block.type === "text" && (
                      <Textarea
                        value={block.text}
                        placeholder={t("notes.editor.body")}
                        aria-label={t("notes.block.text", { index: index + 1 })}
                        className="min-h-[70px]"
                        onChange={(e) =>
                          updateBlock(index, { text: e.target.value } as Partial<NoteBlock>)
                        }
                      />
                    )}

                    {block.type === "bullet" && (
                      <div className="flex items-center gap-2">
                        <span aria-hidden="true" className="text-gray-400">
                          •
                        </span>
                        <Input
                          value={block.text}
                          placeholder={t("notes.editor.listItem")}
                          aria-label={t("notes.block.bullet", { index: index + 1 })}
                          onChange={(e) =>
                            updateBlock(index, { text: e.target.value } as Partial<NoteBlock>)
                          }
                        />
                      </div>
                    )}

                    {block.type === "image" && (
                      <figure className="rounded-lg border bg-gray-50 p-2">
                        <NoteImage path={block.path} alt={block.alt} />
                        <figcaption className="mt-2 flex items-center gap-2">
                          <Input
                            value={block.alt}
                            placeholder={t("notes.editor.caption")}
                            aria-label={t("notes.block.caption", { index: index + 1 })}
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
                    aria-label={t("notes.block.remove", { index: index + 1 })}
                    title={t("common.remove")}
                    onClick={() =>
                      block.type === "image" ? void handleDeleteImage(index) : removeBlock(index)
                    }
                  >
                    {block.type === "image" ? (
                      <X className="h-4 w-4" />
                    ) : (
                      <Trash2 className="h-4 w-4" />
                    )}
                  </Button>
                </div>
              ))}
            </div>

            <div className="mt-6 flex flex-wrap gap-2 border-t pt-4">
              <Button variant="outline" size="sm" onClick={() => addBlock("heading")}>
                <Plus className="mr-2 h-4 w-4" />
                {t("notes.addHeading")}
              </Button>
              <Button variant="outline" size="sm" onClick={() => addBlock("text")}>
                <Plus className="mr-2 h-4 w-4" />
                {t("notes.addText")}
              </Button>
              <Button variant="outline" size="sm" onClick={() => addBlock("bullet")}>
                <Plus className="mr-2 h-4 w-4" />
                {t("notes.addBullet")}
              </Button>
              <Button variant="outline" size="sm" onClick={() => fileInputRef.current?.click()}>
                <ImagePlus className="mr-2 h-4 w-4" />
                {t("notes.addImageBlock")}
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
