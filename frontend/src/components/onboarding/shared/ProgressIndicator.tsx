import React from 'react';
import { Check, Lock, Download, CheckCircle2, BrainCircuit } from 'lucide-react';
import { useI18n } from '@/contexts/I18nContext';

interface ProgressIndicatorProps {
  current: number;
  total: number;
  onStepClick?: (step: number) => void;
}

const stepIcons = [
  Lock,         // 1. Welcome
  BrainCircuit, // 2. Setup Overview
  Download,     // 3. Download Progress
  // Step 4 (Permissions) doesn't need icon - auto-skipped on non-macOS
];

export function ProgressIndicator({ current, total, onStepClick }: ProgressIndicatorProps) {
  const { t } = useI18n();
  const visibleSteps = Array.from({ length: total }, (_, i) => i + 1);

  return (
    <div className="mb-8">
      <div className="flex items-center justify-center gap-2">
        {visibleSteps.map((step, index) => {
          const isActive = step === current;
          const isCompleted = step < current;
          const isClickable = isCompleted && onStepClick;
          const StepIcon = stepIcons[step - 1] || CheckCircle2;

          return (
            <React.Fragment key={step}>
              {/* Step Circle */}
              <button
                type="button"
                onClick={() => isClickable && onStepClick(step)}
                disabled={!isClickable}
                aria-label={t('onboarding.step.label', { current: step, total })}
                aria-current={isActive ? 'step' : undefined}
                className={`relative flex items-center justify-center transition-all duration-300 rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60 ${
                  isCompleted
                    ? 'w-7 h-7 bg-green-600'
                    : isActive
                      ? 'w-8 h-8 bg-gray-900'
                      : 'w-6 h-6 bg-gray-300'
                } ${isClickable ? 'cursor-pointer hover:scale-110 hover:shadow-md' : 'cursor-default'}`}
              >
                {isCompleted ? (
                  <Check className="w-4 h-4 text-white" aria-hidden />
                ) : (
                  <StepIcon
                    aria-hidden
                    className={`transition-all duration-300 ${
                      isActive ? 'w-4 h-4 text-white' : 'w-3 h-3 text-gray-600'
                    }`}
                  />
                )}
              </button>

              {/* Connector Line */}
              {index < visibleSteps.length - 1 && (
                <div
                  aria-hidden
                  className={`h-0.5 w-6 transition-all duration-300 ${
                    isCompleted ? 'bg-green-600' : 'bg-gray-300'
                  }`}
                />
              )}
            </React.Fragment>
          );
        })}
      </div>
    </div>
  );
}
