import type { ReactNode } from "react";
import aidashIcon from "../assets/brand/aidash-app-icon.svg?no-inline";
import { cn } from "../lib/utils";

/** Centered card for sign-in, authority choice and desktop connection screens. */
export function AuthCard({
  title,
  titleId,
  action,
  className,
  children,
}: {
  title: string;
  titleId?: string;
  action?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <main
      aria-labelledby={titleId}
      className={cn(
        "canvas-grid flex min-h-0 flex-1 items-start justify-center overflow-y-auto px-4 py-10 sm:items-center",
        className,
      )}
    >
      <div className="grid w-full max-w-[400px] gap-5 rounded-lg border border-border-strong bg-surface p-6 shadow-overlay">
        <div className="flex items-center gap-2.5">
          <img
            src={aidashIcon}
            alt=""
            width={28}
            height={28}
            className="size-7 rounded-md"
          />
          <span className="text-[13px] font-semibold tracking-tight">
            Aidash
          </span>
          <span className="font-mono text-[11px] text-faint">0.1</span>
          {action && <div className="ml-auto flex gap-1">{action}</div>}
        </div>
        <h1
          id={titleId}
          className="text-[15px] font-semibold leading-snug tracking-tight"
        >
          {title}
        </h1>
        {children}
      </div>
    </main>
  );
}
