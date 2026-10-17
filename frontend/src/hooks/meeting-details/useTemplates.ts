import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { useI18n } from '@/contexts/I18nContext';
import { BUILTIN_TEMPLATE_KEYS, recommendTemplate } from '@/lib/template-recommend';

/** A template as listed by `api_list_templates`. */
export interface TemplateInfo {
  id: string;
  name: string;
  description: string;
  /** User-authored templates can be edited and deleted. */
  is_custom?: boolean;
}

/** A single section within a full template definition. */
export interface TemplateSectionPayload {
  title: string;
  instruction: string;
  /** One of "paragraph" | "list" | "string". */
  format: string;
  item_format?: string | null;
}

/** Full template definition (`api_get_template_full`). */
export interface TemplateFull {
  name: string;
  description: string;
  sections: TemplateSectionPayload[];
}

export interface SaveTemplateResult {
  id: string;
  path: string;
  overwritten: boolean;
}

export function useTemplates() {
  const { t } = useI18n();
  const [rawTemplates, setAvailableTemplates] = useState<TemplateInfo[]>([]);
  /** True once the user picked a template by hand; recommendations then stay out of the way. */
  const userChoseRef = useRef(false);
  const [recommendedTemplate, setRecommendedTemplate] = useState<string | null>(null);

  // Built-in names are shown in the UI language; custom names are the user's own.
  const availableTemplates = useMemo(() => rawTemplates.map((tpl) => {
    const key = !tpl.is_custom ? BUILTIN_TEMPLATE_KEYS[tpl.id] : undefined;
    if (!key) return tpl;
    const name = t(key);
    return name === key ? tpl : { ...tpl, name };
  }), [rawTemplates, t]);
  const [selectedTemplate, setSelectedTemplate] = useState<string>('standard_meeting');
  const [isLoadingTemplates, setIsLoadingTemplates] = useState(false);

  const refreshTemplates = useCallback(async () => {
    setIsLoadingTemplates(true);
    try {
      const templates = await invokeTauri<TemplateInfo[]>('api_list_templates');
      setAvailableTemplates(templates);
      return templates;
    } catch (error) {
      console.error('Failed to fetch templates:', error);
      return [];
    } finally {
      setIsLoadingTemplates(false);
    }
  }, []);

  // Fetch available templates on mount
  useEffect(() => {
    void refreshTemplates();
  }, [refreshTemplates]);

  // Handle template selection
  const handleTemplateSelection = useCallback((templateId: string, templateName: string) => {
    userChoseRef.current = true;
    setSelectedTemplate(templateId);
    toast.success(t('templates.selected'), {
      description: t('templates.selectedDesc', { name: templateName }),
    });
    Analytics.trackFeatureUsed('template_selected');
  }, [t]);

  /**
   * Pick a template from the meeting's title and opening transcript — unless
   * the user already chose one in this view.
   */
  const recommendFor = useCallback((title: string | null | undefined, sample: string | null | undefined) => {
    if (userChoseRef.current || rawTemplates.length === 0) return;
    const id = recommendTemplate(title, sample, rawTemplates.map((tpl) => tpl.id));
    if (!id) return;
    setRecommendedTemplate(id === 'standard_meeting' ? null : id);
    setSelectedTemplate(id);
  }, [rawTemplates]);

  /** Load one template's full definition, for editing. */
  const loadTemplate = useCallback(async (templateId: string): Promise<TemplateFull> => {
    return invokeTauri<TemplateFull>('api_get_template_full', { templateId });
  }, []);

  /**
   * Create or update a custom template.
   *
   * When `templateId` is omitted the backend derives a unique id from the name.
   * `overwrite` must be set to replace an existing custom template.
   */
  const saveTemplate = useCallback(
    async (params: {
      templateId?: string | null;
      name: string;
      description: string;
      sections: TemplateSectionPayload[];
      overwrite?: boolean;
    }): Promise<SaveTemplateResult> => {
      const result = await invokeTauri<SaveTemplateResult>('api_save_template', {
        templateId: params.templateId ?? null,
        name: params.name,
        description: params.description,
        sections: params.sections,
        overwrite: params.overwrite ?? false,
      });

      await refreshTemplates();
      await Analytics.trackFeatureUsed('template_saved');
      toast.success(result.overwritten ? t('templates.updated') : t('templates.created'), {
        description: t('templates.savedAs', { id: result.id }),
      });
      return result;
    },
    [refreshTemplates, t],
  );

  /** Copy any template (built-in included) into a new editable custom template. */
  const duplicateTemplate = useCallback(
    async (templateId: string, newName?: string): Promise<SaveTemplateResult> => {
      const result = await invokeTauri<SaveTemplateResult>('api_duplicate_template', {
        templateId,
        newName: newName ?? null,
      });
      await refreshTemplates();
      toast.success(t('templates.duplicated'), { description: t('templates.savedAs', { id: result.id }) });
      return result;
    },
    [refreshTemplates, t],
  );

  /** Delete a custom template. Built-in templates are refused by the backend. */
  const deleteTemplate = useCallback(
    async (templateId: string): Promise<void> => {
      await invokeTauri('api_delete_template', { templateId });
      await refreshTemplates();
      await Analytics.trackFeatureUsed('template_deleted');
      toast.success(t('templates.deleted'), { description: templateId });
    },
    [refreshTemplates, t],
  );

  /** Directory where custom templates are written (shown in the editor). */
  const getCustomTemplatesDir = useCallback(async (): Promise<string> => {
    return invokeTauri<string>('api_get_custom_templates_dir');
  }, []);

  return {
    availableTemplates,
    selectedTemplate,
    recommendedTemplate,
    recommendFor,
    isLoadingTemplates,
    handleTemplateSelection,
    refreshTemplates,
    loadTemplate,
    saveTemplate,
    duplicateTemplate,
    deleteTemplate,
    getCustomTemplatesDir,
  };
}
