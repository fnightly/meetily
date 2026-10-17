'use client';

import { motion } from 'framer-motion';
import { ListChecks, Sparkles, Target, Lightbulb } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface EmptyStateSummaryProps {
  onGenerate: () => void;
  hasModel: boolean;
  isGenerating?: boolean;
  error?: string | null;
}

export function EmptyStateSummary({
  onGenerate,
  hasModel,
  isGenerating = false,
  error = null,
}: EmptyStateSummaryProps) {
  const { t } = useI18n();
  const benefits = [
    { icon: Lightbulb, label: t('summaryEmpty.b1') },
    { icon: Target, label: t('summaryEmpty.b2') },
    { icon: ListChecks, label: t('summaryEmpty.b3') },
  ];
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col items-center justify-center h-full p-8 text-center"
    >
      <span className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-gradient-to-br from-violet-50 to-blue-50 text-violet-600 ring-1 ring-violet-100">
        <Sparkles className="h-6 w-6" />
      </span>
      <h3 className="text-base font-semibold text-slate-900 mb-1.5">
        {t('summaryEmpty.title')}
      </h3>
      <p className="text-sm text-slate-500 mb-5 max-w-sm leading-relaxed">
        {t('summaryEmpty.body')}
      </p>
      <ul className="mb-6 flex flex-wrap justify-center gap-2">
        {benefits.map(({ icon: Icon, label }) => (
          <li key={label} className="chip bg-slate-100 text-slate-600 px-2.5 py-1 text-xs">
            <Icon className="h-3.5 w-3.5 text-slate-400" />{label}
          </li>
        ))}
      </ul>

      {error && (
        <p role="alert" className="mb-4 max-w-md rounded-md bg-red-50 px-3 py-2 text-sm text-red-700">
          {error}
        </p>
      )}

      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <div>
              <Button
                onClick={onGenerate}
                disabled={!hasModel || isGenerating}
                className="gap-2 h-10 px-5 rounded-lg bg-violet-600 hover:bg-violet-700"
              >
                <Sparkles className="w-4 h-4" />
                {isGenerating ? t('summaryEmpty.generating') : error ? t('summaryEmpty.retry') : t('summaryEmpty.generate')}
              </Button>
            </div>
          </TooltipTrigger>
          {!hasModel && (
            <TooltipContent>
              <p>{t('summaryEmpty.needModel')}</p>
            </TooltipContent>
          )}
        </Tooltip>
      </TooltipProvider>

      {!hasModel && (
        <p className="text-xs text-amber-600 mt-3">
          {t('summaryEmpty.needModel')}
        </p>
      )}
    </motion.div>
  );
}
