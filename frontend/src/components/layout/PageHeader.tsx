import type { ReactNode } from 'react';

interface PageHeaderProps {
  title: string;
  description?: string;
  icon?: ReactNode;
  actions?: ReactNode;
  /** Constrain to the same width as the page body. */
  maxWidthClass?: string;
}

/** One header pattern for every top-level page: icon tile, title, one line of context, actions. */
export function PageHeader({ title, description, icon, actions, maxWidthClass = 'max-w-6xl' }: PageHeaderProps) {
  return (
    <header className="flex-shrink-0 border-b border-slate-200 bg-white">
      <div className={`mx-auto flex w-full items-center gap-4 px-8 py-5 ${maxWidthClass}`}>
        {icon && (
          <span className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-xl bg-blue-50 text-blue-600 [&_svg]:h-5 [&_svg]:w-5">
            {icon}
          </span>
        )}
        <div className="min-w-0 flex-1">
          <h1 className="text-xl font-semibold tracking-tight text-slate-900">{title}</h1>
          {description && <p className="mt-0.5 text-sm text-slate-500">{description}</p>}
        </div>
        {actions && <div className="flex flex-shrink-0 items-center gap-2">{actions}</div>}
      </div>
    </header>
  );
}
