import React from "react";
import { Info as InfoIcon } from "lucide-react";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./ui/dialog";
import { VisuallyHidden } from "./ui/visually-hidden";
import { About } from "./About";
import { tr } from "@/lib/i18n";

interface InfoProps {
    isCollapsed: boolean;
}

const Info = React.forwardRef<HTMLButtonElement, InfoProps>(({ isCollapsed }, ref) => {
  return (
    <Dialog aria-describedby={undefined}>
      <DialogTrigger asChild>
        <button 
          ref={ref} 
          className={`flex items-center justify-center cursor-pointer border-none transition-colors ${
            isCollapsed 
              ? "bg-transparent h-9 w-9 text-slate-500 hover:bg-slate-100 hover:text-slate-900 rounded-lg" 
              : "w-full px-3 py-1.5 mt-1 text-sm font-medium text-gray-700 bg-gray-200 hover:bg-gray-200 rounded-lg shadow-sm"
          }`}
          title={tr('common.about')}
          aria-label={tr('common.about')}
        >
          <InfoIcon className={isCollapsed ? "w-[18px] h-[18px]" : "w-4 h-4 text-gray-600"} />
          {!isCollapsed && (
            <span className="ml-2 text-sm text-gray-700">{tr('common.about')}</span>
          )}
        </button>
      </DialogTrigger>
      <DialogContent>
        <VisuallyHidden>
          <DialogTitle>{tr('common.about')}</DialogTitle>
        </VisuallyHidden>
        <About />
      </DialogContent>
    </Dialog>
  );
});

Info.displayName = "About";

export default Info; 