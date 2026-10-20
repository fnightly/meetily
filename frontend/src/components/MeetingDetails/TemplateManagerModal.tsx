"use client";

import { useCallback, useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Plus, Pencil, Copy, Trash2, ChevronUp, ChevronDown, ArrowLeft } from "lucide-react";
import { toast } from "sonner";
import type {
  TemplateDetailLevel,
  TemplateGranularity,
  TemplateInfo,
  TemplateSectionPayload,
} from "@/hooks/meeting-details/useTemplates";

/**
 * Create, edit, duplicate and delete summary templates.
 *
 * Built-in templates are read-only (they are embedded in the binary), so the UI
 * offers "Duplicate" for them and full editing only for custom ones. The backend
 * enforces the same rule, so a race here cannot corrupt a built-in template.
 */

const FORMATS = [
  { value: "paragraph", label: "Paragraph" },
  { value: "list", label: "List" },
  { value: "string", label: "Short text" },
];

const DETAIL_LEVELS: Array<{ value: TemplateDetailLevel; label: string }> = [
  { value: "brief", label: "Brief — essentials only" },
  { value: "standard", label: "Standard — balanced" },
  { value: "detailed", label: "Detailed — context and reasoning" },
];

const GRANULARITIES: Array<{ value: TemplateGranularity; label: string }> = [
  { value: "coarse", label: "Coarse — grouped themes" },
  { value: "balanced", label: "Balanced — decisions and steps" },
  { value: "fine", label: "Fine — every point, decision and owner" },
];

interface SectionDraft {
  title: string;
  instruction: string;
  format: string;
  item_format: string;
}

interface TemplateDraft {
  /** null when creating a new template. */
  id: string | null;
  name: string;
  description: string;
  detailLevel: TemplateDetailLevel;
  granularity: TemplateGranularity;
  sections: SectionDraft[];
}

function emptySection(): SectionDraft {
  return { title: "", instruction: "", format: "paragraph", item_format: "" };
}

function emptyDraft(): TemplateDraft {
  return {
    id: null,
    name: "",
    description: "",
    detailLevel: "standard",
    granularity: "balanced",
    sections: [emptySection()],
  };
}

export interface TemplateManagerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  templates: TemplateInfo[];
  loadTemplate: (id: string) => Promise<{
    name: string;
    description: string;
    detail_level?: TemplateDetailLevel | null;
    granularity?: TemplateGranularity | null;
    sections: TemplateSectionPayload[];
  }>;
  saveTemplate: (params: {
    templateId?: string | null;
    name: string;
    description: string;
    detail_level?: TemplateDetailLevel | null;
    granularity?: TemplateGranularity | null;
    sections: TemplateSectionPayload[];
    overwrite?: boolean;
  }) => Promise<{ id: string; overwritten: boolean }>;
  duplicateTemplate: (id: string, newName?: string) => Promise<{ id: string }>;
  deleteTemplate: (id: string) => Promise<void>;
  getCustomTemplatesDir: () => Promise<string>;
}

export function TemplateManagerModal({
  open,
  onOpenChange,
  templates,
  loadTemplate,
  saveTemplate,
  duplicateTemplate,
  deleteTemplate,
  getCustomTemplatesDir,
}: TemplateManagerModalProps) {
  const [draft, setDraft] = useState<TemplateDraft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [templatesDir, setTemplatesDir] = useState<string>("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setDraft(null);
      setError(null);
      setConfirmDeleteId(null);
      return;
    }
    getCustomTemplatesDir()
      .then(setTemplatesDir)
      .catch(() => setTemplatesDir(""));
  }, [open, getCustomTemplatesDir]);

  const startCreate = useCallback(() => {
    setError(null);
    setDraft(emptyDraft());
  }, []);

  const startEdit = useCallback(
    async (id: string) => {
      setBusy(true);
      setError(null);
      try {
        const full = await loadTemplate(id);
        setDraft({
          id,
          name: full.name,
          description: full.description,
          detailLevel: full.detail_level ?? "standard",
          granularity: full.granularity ?? "balanced",
          sections:
            full.sections.length > 0
              ? full.sections.map((s) => ({
                  title: s.title,
                  instruction: s.instruction,
                  format: s.format || "paragraph",
                  item_format: s.item_format ?? "",
                }))
              : [emptySection()],
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [loadTemplate],
  );

  const handleDuplicate = useCallback(
    async (id: string) => {
      setBusy(true);
      setError(null);
      try {
        await duplicateTemplate(id);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [duplicateTemplate],
  );

  const handleDelete = useCallback(
    async (id: string) => {
      setBusy(true);
      setError(null);
      try {
        await deleteTemplate(id);
        setConfirmDeleteId(null);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [deleteTemplate],
  );

  const updateSection = useCallback(
    (index: number, patch: Partial<SectionDraft>) => {
      setDraft((prev) => {
        if (!prev) return prev;
        const sections = prev.sections.map((s, i) => (i === index ? { ...s, ...patch } : s));
        return { ...prev, sections };
      });
    },
    [],
  );

  const moveSection = useCallback((index: number, delta: number) => {
    setDraft((prev) => {
      if (!prev) return prev;
      const target = index + delta;
      if (target < 0 || target >= prev.sections.length) return prev;
      const sections = [...prev.sections];
      const [item] = sections.splice(index, 1);
      sections.splice(target, 0, item);
      return { ...prev, sections };
    });
  }, []);

  const removeSection = useCallback((index: number) => {
    setDraft((prev) => {
      if (!prev) return prev;
      // Always keep at least one row so the form stays usable.
      if (prev.sections.length <= 1) return { ...prev, sections: [emptySection()] };
      return { ...prev, sections: prev.sections.filter((_, i) => i !== index) };
    });
  }, []);

  const handleSave = useCallback(async () => {
    if (!draft) return;

    // Client-side checks mirror the backend validator so the user gets an
    // immediate, specific message instead of a round-trip failure.
    const name = draft.name.trim();
    const description = draft.description.trim();
    if (!name) return setError("Template name is required.");
    if (!description) return setError("Template description is required.");

    const sections: TemplateSectionPayload[] = draft.sections.map((s) => ({
      title: s.title.trim(),
      instruction: s.instruction.trim(),
      format: s.format,
      item_format: s.item_format.trim() ? s.item_format.trim() : null,
    }));

    if (sections.length === 0) return setError("Add at least one section.");
    const emptyIndex = sections.findIndex((s) => !s.title || !s.instruction);
    if (emptyIndex >= 0) {
      return setError(`Section ${emptyIndex + 1} needs both a title and an instruction.`);
    }

    setBusy(true);
    setError(null);
    try {
      await saveTemplate({
        templateId: draft.id,
        name,
        description,
        detail_level: draft.detailLevel,
        granularity: draft.granularity,
        sections,
        // Editing an existing custom template replaces it; creating never does.
        overwrite: draft.id !== null,
      });
      setDraft(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [draft, saveTemplate]);

  const customCount = templates.filter((t) => t.is_custom).length;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl max-h-[85vh] overflow-y-auto" aria-describedby={undefined}>
        <DialogHeader>
          <DialogTitle>
            {draft ? (draft.id ? "Edit template" : "New template") : "Summary templates"}
          </DialogTitle>
          <DialogDescription>
            {draft
              ? "Sections are generated in the order listed. The instruction tells the AI what to extract for that section."
              : `${templates.length} templates available (${customCount} custom). Built-in templates can be duplicated and then customised.`}
          </DialogDescription>
        </DialogHeader>

        {error && (
          <div role="alert" className="p-3 bg-red-50 border border-red-200 rounded-md">
            <p className="text-xs text-red-800">{error}</p>
          </div>
        )}

        {/* ---------- List view ---------- */}
        {!draft && (
          <div className="space-y-3">
            <div className="flex justify-end">
              <Button size="sm" onClick={startCreate}>
                <Plus className="w-4 h-4 mr-2" />
                New template
              </Button>
            </div>

            {templates.length === 0 && (
              <p className="text-sm text-gray-600">No templates found.</p>
            )}

            <ul className="space-y-2">
              {templates.map((t) => (
                <li
                  key={t.id}
                  className="flex items-start justify-between gap-3 p-3 border rounded-lg"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-medium truncate">{t.name}</span>
                      <span
                        className={`text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded ${
                          t.is_custom
                            ? "bg-blue-100 text-blue-800"
                            : "bg-gray-200 text-gray-700"
                        }`}
                      >
                        {t.is_custom ? "Custom" : "Built-in"}
                      </span>
                    </div>
                    <p className="text-xs text-gray-600 mt-1">{t.description}</p>
                    <p className="text-[10px] text-gray-400 font-mono mt-1">{t.id}</p>
                  </div>

                  <div className="flex items-center gap-1 shrink-0">
                    {t.is_custom && (
                      <Button
                        variant="outline"
                        size="sm"
                        title="Edit"
                        aria-label={`Edit ${t.name}`}
                        disabled={busy}
                        onClick={() => startEdit(t.id)}
                      >
                        <Pencil className="w-4 h-4" />
                      </Button>
                    )}
                    <Button
                      variant="outline"
                      size="sm"
                      title="Duplicate"
                      aria-label={`Duplicate ${t.name}`}
                      disabled={busy}
                      onClick={() => handleDuplicate(t.id)}
                    >
                      <Copy className="w-4 h-4" />
                    </Button>
                    {t.is_custom &&
                      (confirmDeleteId === t.id ? (
                        <>
                          <Button
                            variant="outline"
                            size="sm"
                            className="text-red-700 border-red-300"
                            disabled={busy}
                            onClick={() => handleDelete(t.id)}
                          >
                            Confirm
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={busy}
                            onClick={() => setConfirmDeleteId(null)}
                          >
                            Cancel
                          </Button>
                        </>
                      ) : (
                        <Button
                          variant="outline"
                          size="sm"
                          title="Delete"
                          aria-label={`Delete ${t.name}`}
                          disabled={busy}
                          onClick={() => setConfirmDeleteId(t.id)}
                        >
                          <Trash2 className="w-4 h-4" />
                        </Button>
                      ))}
                  </div>
                </li>
              ))}
            </ul>

            {templatesDir && (
              <p className="text-[11px] text-gray-500 break-all">
                Custom templates are stored in <span className="font-mono">{templatesDir}</span>
              </p>
            )}
          </div>
        )}

        {/* ---------- Editor view ---------- */}
        {draft && (
          <div className="space-y-4">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setDraft(null)}
              disabled={busy}
              className="-ml-2"
            >
              <ArrowLeft className="w-4 h-4 mr-2" />
              Back to list
            </Button>

            <div className="space-y-2">
              <Label htmlFor="tpl-name">Template name</Label>
              <Input
                id="tpl-name"
                value={draft.name}
                placeholder="e.g. Weekly engineering sync"
                onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="tpl-desc">Description</Label>
              <Input
                id="tpl-desc"
                value={draft.description}
                placeholder="What this template is for"
                onChange={(e) => setDraft({ ...draft, description: e.target.value })}
              />
            </div>

            {/* Template-wide breadth controls: they are applied to every section
                through the generated section instructions. */}
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="tpl-detail">Detail level</Label>
                <Select
                  value={draft.detailLevel}
                  onValueChange={(value) =>
                    setDraft({ ...draft, detailLevel: value as TemplateDetailLevel })
                  }
                >
                  <SelectTrigger id="tpl-detail">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {DETAIL_LEVELS.map((level) => (
                      <SelectItem key={level.value} value={level.value}>
                        {level.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <div className="space-y-2">
                <Label htmlFor="tpl-granularity">Granularity</Label>
                <Select
                  value={draft.granularity}
                  onValueChange={(value) =>
                    setDraft({ ...draft, granularity: value as TemplateGranularity })
                  }
                >
                  <SelectTrigger id="tpl-granularity">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {GRANULARITIES.map((level) => (
                      <SelectItem key={level.value} value={level.value}>
                        {level.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <Label>Sections</Label>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setDraft({ ...draft, sections: [...draft.sections, emptySection()] })
                  }
                >
                  <Plus className="w-4 h-4 mr-2" />
                  Add section
                </Button>
              </div>

              {draft.sections.map((section, index) => (
                <div key={index} className="p-3 border rounded-lg space-y-3 bg-gray-50">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-medium text-gray-600">
                      Section {index + 1}
                    </span>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`Move section ${index + 1} up`}
                        disabled={index === 0}
                        onClick={() => moveSection(index, -1)}
                      >
                        <ChevronUp className="w-4 h-4" />
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`Move section ${index + 1} down`}
                        disabled={index === draft.sections.length - 1}
                        onClick={() => moveSection(index, 1)}
                      >
                        <ChevronDown className="w-4 h-4" />
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`Remove section ${index + 1}`}
                        onClick={() => removeSection(index)}
                      >
                        <Trash2 className="w-4 h-4" />
                      </Button>
                    </div>
                  </div>

                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                    <div className="sm:col-span-2 space-y-1">
                      <Label htmlFor={`sec-title-${index}`}>Title</Label>
                      <Input
                        id={`sec-title-${index}`}
                        value={section.title}
                        placeholder="e.g. Action Items"
                        onChange={(e) => updateSection(index, { title: e.target.value })}
                      />
                    </div>
                    <div className="space-y-1">
                      <Label htmlFor={`sec-format-${index}`}>Format</Label>
                      <Select
                        value={section.format}
                        onValueChange={(value) => updateSection(index, { format: value })}
                      >
                        <SelectTrigger id={`sec-format-${index}`}>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {FORMATS.map((f) => (
                            <SelectItem key={f.value} value={f.value}>
                              {f.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </div>

                  <div className="space-y-1">
                    <Label htmlFor={`sec-instruction-${index}`}>Instruction for the AI</Label>
                    <Textarea
                      id={`sec-instruction-${index}`}
                      value={section.instruction}
                      placeholder="e.g. List every decision made, with who owns it"
                      className="min-h-[70px]"
                      onChange={(e) => updateSection(index, { instruction: e.target.value })}
                    />
                  </div>

                  <div className="space-y-1">
                    <Label htmlFor={`sec-item-${index}`}>Item format (optional)</Label>
                    <Input
                      id={`sec-item-${index}`}
                      value={section.item_format}
                      placeholder="e.g. [Owner] - [Task] - [Due date]"
                      onChange={(e) => updateSection(index, { item_format: e.target.value })}
                    />
                  </div>
                </div>
              ))}
            </div>

            <div className="flex justify-end gap-2 pt-2">
              <Button variant="outline" onClick={() => setDraft(null)} disabled={busy}>
                Cancel
              </Button>
              <Button onClick={handleSave} disabled={busy}>
                {busy ? "Saving…" : draft.id ? "Save changes" : "Create template"}
              </Button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
