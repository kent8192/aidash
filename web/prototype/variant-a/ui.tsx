// PROTOTYPE ONLY. Shared primitives for variant A "Signal Desk".
import type { ReactNode } from "react";

export type Tone = "accent" | "done" | "warn" | "fail" | "idle";

export const toneColor: Record<Tone, string> = {
  accent: "var(--a-accent)",
  done: "var(--a-done)",
  warn: "var(--a-warn)",
  fail: "var(--a-fail)",
  idle: "#a3abb8",
};

export function statusTone(status: string): Tone {
  switch (status) {
    case "RUNNING":
    case "THINKING":
    case "TOOL_CALL":
    case "CLAIMED":
      return "accent";
    case "COMPLETED":
    case "allow":
    case "resolved":
      return "done";
    case "BLOCKED":
    case "WAITING":
    case "pending":
    case "require-approval":
    case "medium":
      return "warn";
    case "FAILED":
    case "deny":
    case "high":
      return "fail";
    default:
      return "idle";
  }
}

/** Small filled square status glyph. */
export function Glyph({ tone, size = 7, hollow = false }: { tone: Tone; size?: number; hollow?: boolean }) {
  return (
    <span
      aria-hidden
      className="inline-block shrink-0"
      style={{
        width: size,
        height: size,
        background: hollow ? "transparent" : toneColor[tone],
        boxShadow: hollow ? `inset 0 0 0 1.5px ${toneColor[tone]}` : undefined,
      }}
    />
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="a-kbd">{children}</kbd>;
}

export const NOW = new Date("2026-10-10T09:42:00+09:00");

export function relativeTime(iso: string) {
  const minutes = Math.floor((NOW.getTime() - new Date(iso).getTime()) / 60000);
  if (minutes < 60) return `${minutes}分前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}時間前`;
  return `${Math.floor(hours / 24)}日前`;
}

/** "09:14" -> minutes since 00:00 */
export const toMinutes = (hhmm: string) => {
  const [h, m, s] = hhmm.split(":").map(Number);
  return h * 60 + m + (s ?? 0) / 60;
};

export const NOW_MIN = toMinutes("09:42");

export function duration(start: string, end: string | null) {
  const m = Math.round((end ? toMinutes(end) : NOW_MIN) - toMinutes(start));
  return `${m}m`;
}

export const nodeLabel = (uri: string) => uri.replace("aidash://", "");

export function Pane({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return <section className={`flex min-h-0 min-w-0 flex-col ${className}`}>{children}</section>;
}

export function PaneHeader({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <header
      className={`flex h-11 shrink-0 items-center gap-3 border-b border-(--a-line) px-4 ${className}`}
    >
      {children}
    </header>
  );
}

export function SectionTitle({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="mb-2 flex items-baseline justify-between gap-3">
      <h3 className="a-eyebrow m-0">{children}</h3>
      {aside ? <span className="mono text-[12px] text-(--a-faint)">{aside}</span> : null}
    </div>
  );
}
