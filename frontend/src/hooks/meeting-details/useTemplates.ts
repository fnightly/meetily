import { useState, useEffect, useCallback } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';

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
  const [availableTemplates, setAvailableTemplates] = useState<TemplateInfo[]>([]);
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
    setSelectedTemplate(templateId);
    toast.success('Template selected', {
      description: `Using "${templateName}" template for summary generation`,
    });
    Analytics.trackFeatureUsed('template_selected');
  }, []);

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
      toast.success(result.overwritten ? 'Template updated' : 'Template created', {
        description: `Saved as "${result.id}"`,
      });
      return result;
    },
    [refreshTemplates],
  );

  /** Copy any template (built-in included) into a new editable custom template. */
  const duplicateTemplate = useCallback(
    async (templateId: string, newName?: string): Promise<SaveTemplateResult> => {
      const result = await invokeTauri<SaveTemplateResult>('api_duplicate_template', {
        templateId,
        newName: newName ?? null,
      });
      await refreshTemplates();
      toast.success('Template duplicated', { description: `Saved as "${result.id}"` });
      return result;
    },
    [refreshTemplates],
  );

  /** Delete a custom template. Built-in templates are refused by the backend. */
  const deleteTemplate = useCallback(
    async (templateId: string): Promise<void> => {
      await invokeTauri('api_delete_template', { templateId });
      await refreshTemplates();
      await Analytics.trackFeatureUsed('template_deleted');
      toast.success('Template deleted', { description: templateId });
    },
    [refreshTemplates],
  );

  /** Directory where custom templates are written (shown in the editor). */
  const getCustomTemplatesDir = useCallback(async (): Promise<string> => {
    return invokeTauri<string>('api_get_custom_templates_dir');
  }, []);

  return {
    availableTemplates,
    selectedTemplate,
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
