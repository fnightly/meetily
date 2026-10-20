import React from 'react';
import { CheckCircle2, Loader2, XCircle } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { useI18n } from '@/contexts/I18nContext';
import type { PermissionRowProps } from '@/types/onboarding';

export function PermissionRow({ icon, title, description, status, isPending = false, onAction }: PermissionRowProps) {
  const { t } = useI18n();
  const isAuthorized = status === 'authorized';
  const isDenied = status === 'denied';
  const isChecking = isPending;

  const getButtonText = () => {
    if (isChecking) return t('onboarding.permissions.checking');
    if (isDenied) return t('onboarding.permissions.openSettings');
    return t('onboarding.permissions.enable');
  };

  return (
    <div
      className={cn(
        'flex items-center justify-between rounded-2xl border px-6 py-5',
        'transition-all duration-200',
        isAuthorized ? 'border-gray-900 bg-gray-100' : isDenied ? 'border-red-300 bg-red-50' : 'bg-white border-neutral-200'
      )}
    >
      {/* Left side: Icon + Info */}
      <div className="flex items-center gap-3 flex-1 min-w-0">
        {/* Icon */}
        <div
          className={cn(
            'flex size-10 items-center justify-center rounded-full flex-shrink-0',
            isAuthorized ? 'bg-gray-200' : isDenied ? 'bg-red-100' : 'bg-neutral-50'
          )}
        >
          <div className={cn(isAuthorized ? 'text-gray-900' : isDenied ? 'text-red-500' : 'text-neutral-500')}>{icon}</div>
        </div>

        {/* Title + Description */}
        <div className="min-w-0 flex-1">
          <div className="font-medium truncate text-neutral-900">{title}</div>
          <div className="text-sm text-muted-foreground" aria-live="polite">
            {isAuthorized ? (
              <span className="text-green-600 flex items-center gap-1">
                <CheckCircle2 className="w-3.5 h-3.5" aria-hidden />
                {t('onboarding.permissions.granted')}
              </span>
            ) : isDenied ? (
              <span className="text-red-500 flex items-center gap-1">
                <XCircle className="w-3.5 h-3.5" aria-hidden />
                {t('onboarding.permissions.denied')}
              </span>
            ) : (
              <span>{description}</span>
            )}
          </div>
        </div>
      </div>

      {/* Right side: Action button or checkmark */}
      <div className="flex items-center gap-2 flex-shrink-0 ml-3">
        {!isAuthorized && (
          <Button
            variant={isDenied ? "destructive" : "outline"}
            size="sm"
            onClick={onAction}
            disabled={isChecking}
            aria-busy={isChecking}
            className="min-w-[100px]"
          >
            {isChecking && <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden />}
            {getButtonText()}
          </Button>
        )}
        {isAuthorized && (
          <div className="flex size-8 items-center justify-center rounded-full bg-green-100">
            <CheckCircle2 className="w-4 h-4 text-green-600" aria-hidden />
          </div>
        )}
      </div>
    </div>
  );
}
