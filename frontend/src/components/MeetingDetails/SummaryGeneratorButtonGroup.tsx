"use client";
import { useI18n } from '@/contexts/I18nContext';

import { ModelConfig, ModelSettingsModal } from '@/components/ModelSettingsModal';
import {
  Dialog,
  DialogContent,
  DialogTrigger,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Sparkles, Settings, Loader2, FileText, Check, Square, SlidersHorizontal } from 'lucide-react';
import { TemplateManagerModal } from './TemplateManagerModal';
import type { TemplateInfo } from '@/hooks/meeting-details/useTemplates';
import { useState, useEffect, ReactNode } from 'react';

export interface SummaryGeneratorButtonGroupProps {
  languageSlot?: ReactNode;
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  availableTemplates: TemplateInfo[];
  selectedTemplate: string;
  /** Suggested for this meeting; badged in the list. */
  recommendedTemplate?: string | null;
  onTemplateSelect: (templateId: string, templateName: string) => void;
  hasTranscripts?: boolean;
  hasSummary?: boolean;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
  /** Template management (optional: the selector still works without it). */
  templateActions?: {
    loadTemplate: (id: string) => Promise<{ name: string; description: string; sections: Array<{ title: string; instruction: string; format: string; item_format?: string | null }> }>;
    saveTemplate: (params: {
      templateId?: string | null;
      name: string;
      description: string;
      sections: Array<{ title: string; instruction: string; format: string; item_format?: string | null }>;
      overwrite?: boolean;
    }) => Promise<{ id: string; overwritten: boolean }>;
    duplicateTemplate: (id: string, newName?: string) => Promise<{ id: string }>;
    deleteTemplate: (id: string) => Promise<void>;
    getCustomTemplatesDir: () => Promise<string>;
  };
}

export function SummaryGeneratorButtonGroup({
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  summaryStatus,
  availableTemplates,
  selectedTemplate,
  onTemplateSelect,
  hasTranscripts = true,
  hasSummary = false,
  isModelConfigLoading = false,
  onOpenModelSettings,
  templateActions,
  recommendedTemplate = null,
  languageSlot
}: SummaryGeneratorButtonGroupProps) {
  const { t } = useI18n();
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [templateManagerOpen, setTemplateManagerOpen] = useState(false);

  // Expose the function to open the modal via callback registration
  useEffect(() => {
    if (onOpenModelSettings) {
      // Register our open dialog function with the parent by calling the callback
      // This allows the parent to store a reference to this function
      const openDialog = () => {
        console.log('📱 Opening model settings dialog via callback');
        setSettingsDialogOpen(true);
      };

      // Call the parent's callback with our open function
      // Note: This assumes onOpenModelSettings accepts a function parameter
      // We'll need to adjust the signature
      onOpenModelSettings(openDialog);
    }
  }, [onOpenModelSettings]);

  if (!hasTranscripts) {
    return null;
  }

  const isGenerating = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';

  return (
    <ButtonGroup>
      {/* Generate Summary or Stop button */}
      {isGenerating ? (
        <Button
          variant="outline"
          size="sm"
          className="bg-red-50 text-red-700 hover:bg-red-100 border-red-200 px-3 gap-2"
          onClick={() => {
            onStopGeneration();
          }}
          title={t('sumBtn.stopHint')}
        >
          <Square size={18} fill="currentColor" />
          <span className="hidden @[24rem]:inline">{t('sumBtn.stop')}</span>
        </Button>
      ) : (
        <Button
          variant="outline"
          size="sm"
          className="bg-violet-600 text-white hover:bg-violet-700 hover:text-white border-violet-600 px-3 gap-2"
          onClick={() => {
            void onGenerateSummary(customPrompt);
          }}
          disabled={isModelConfigLoading}
          title={
            isModelConfigLoading
              ? t('sumBtn.loadingModel')
              : hasSummary ? t('sumBtn.regenerate') : t('sumBtn.generate')
          }
        >
          {isModelConfigLoading ? (
            <>
              <Loader2 className="animate-spin" size={18} />
              <span className="hidden @[24rem]:inline">{t('sumBtn.processing')}</span>
            </>
          ) : (
            <>
              <Sparkles size={18} />
              <span className="hidden @[24rem]:inline">{hasSummary ? t('sumBtn.regenerate') : t('sumBtn.generate')}</span>
            </>
          )}
        </Button>
      )}

      {languageSlot}

      {/* Settings button */}
      <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
        <DialogTrigger asChild>
          <Button
            variant="outline"
            size="sm"
            title={t('sumBtn.model')}
          >
            <Settings />
            <span className="hidden @[40rem]:inline">{t('sumBtn.model')}</span>
          </Button>
        </DialogTrigger>
        <DialogContent
          aria-describedby={undefined}
        >
          <VisuallyHidden>
            <DialogTitle>{t('sumBtn.model')}</DialogTitle>
          </VisuallyHidden>
          <ModelSettingsModal
            onSave={async (config) => {
              await onSaveModelConfig(config);
              setSettingsDialogOpen(false);
            }}
            modelConfig={modelConfig}
            setModelConfig={setModelConfig}
            skipInitialFetch={true}
            layout="dialog"
          />
        </DialogContent>
      </Dialog>

      {/* Template selector dropdown */}
      {availableTemplates.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              title={t('sumBtn.template')}
            >
              <FileText />
              <span className="hidden @[40rem]:inline max-w-[9rem] truncate">
                {availableTemplates.find((tpl) => tpl.id === selectedTemplate)?.name ?? t('sumBtn.template')}
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {availableTemplates.map((template) => (
              <DropdownMenuItem
                key={template.id}
                onClick={() => onTemplateSelect(template.id, template.name)}
                title={template.description}
                className="flex items-center justify-between gap-2"
              >
                <span className="flex items-center gap-1.5">
                  {template.name}
                  {recommendedTemplate === template.id && (
                    <span className="rounded bg-violet-50 px-1.5 py-0.5 text-[10px] font-semibold text-violet-700">{t('templates.recommended')}</span>
                  )}
                </span>
                {selectedTemplate === template.id && (
                  <Check className="h-4 w-4 text-green-600" />
                )}
              </DropdownMenuItem>
            ))}

            {templateActions && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={() => {
                    setTemplateManagerOpen(true);
                  }}
                  className="flex items-center gap-2"
                >
                  <SlidersHorizontal className="h-4 w-4" />
                  <span>{t('templates.manage')}</span>
                </DropdownMenuItem>
              </>
            )}

          </DropdownMenuContent>
        </DropdownMenu>
      )}

      {templateActions && (
        <TemplateManagerModal
          open={templateManagerOpen}
          onOpenChange={setTemplateManagerOpen}
          templates={availableTemplates}
          loadTemplate={templateActions.loadTemplate}
          saveTemplate={templateActions.saveTemplate}
          duplicateTemplate={templateActions.duplicateTemplate}
          deleteTemplate={templateActions.deleteTemplate}
          getCustomTemplatesDir={templateActions.getCustomTemplatesDir}
        />
      )}
    </ButtonGroup>
  );
}
