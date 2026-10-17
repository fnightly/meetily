import React from "react";
import Image from "next/image";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./ui/dialog";
import { VisuallyHidden } from "./ui/visually-hidden";
import { About } from "./About";
import { tr } from "@/lib/i18n";

interface LogoProps {
  isCollapsed: boolean;
}

const Logo = React.forwardRef<HTMLButtonElement, LogoProps>(
  ({ isCollapsed }, ref) => {
    return (
      <Dialog aria-describedby={undefined}>
        {isCollapsed ? (
          <DialogTrigger asChild>
            <button
              ref={ref}
              type="button"
              className="flex items-center justify-center mb-1 cursor-pointer bg-transparent border-none p-0 hover:opacity-80 transition-opacity"
              aria-label="About Meetily"
            >
              <Image
                src="/logo-collapsed.png"
                alt="Meetily"
                width={34}
                height={34}
                className="object-contain"
                priority
              />
            </button>
          </DialogTrigger>
        ) : (
          <DialogTrigger asChild>
            <button
              ref={ref}
              type="button"
              className="flex w-full items-center gap-2.5 rounded-lg p-1 text-left hover:bg-slate-50 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
              aria-label="About Meetily"
            >
              <Image src="/logo-collapsed.png" alt="" width={28} height={28} className="object-contain" priority />
              <span className="min-w-0">
                <span className="block text-[15px] font-bold leading-5 tracking-tight text-slate-900">Meetily</span>
                <span className="block text-[11px] leading-4 text-slate-400">{tr('nav.tagline')}</span>
              </span>
            </button>
          </DialogTrigger>
        )}
        <DialogContent>
          <VisuallyHidden>
            <DialogTitle>About Meetily</DialogTitle>
          </VisuallyHidden>
          <About />
        </DialogContent>
      </Dialog>
    );
  },
);

Logo.displayName = "Logo";

export default Logo;
