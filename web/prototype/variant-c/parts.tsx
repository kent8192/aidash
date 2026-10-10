// PROTOTYPE ONLY. Variant C shared bits: avatars, status words, small derivations.
import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { agentName, viewer } from "../data";

/** Quiet breadcrumb strip at the top of every document; no heavy app bar. */
export function TopStrip({
  crumbs,
  note,
  children,
}: {
  crumbs: { label: string; onClick?: () => void }[];
  note?: string;
  children?: ReactNode;
}) {
  return (
    <header className="c-top">
      <nav className="c-crumbs" aria-label="パンくず">
        <button type="button">{viewer.tenant}</button>
        {crumbs.map((crumb, index) => (
          <span key={crumb.label} className="contents">
            <ChevronRight size={13} />
            {index === crumbs.length - 1 ? (
              <span aria-current="page" className="px-1">
                {crumb.label}
              </span>
            ) : (
              <button type="button" onClick={crumb.onClick}>
                {crumb.label}
              </button>
            )}
          </span>
        ))}
      </nav>
      <div className="c-top__actions">
        {note && <span className="c-top__note">{note}</span>}
        {children}
      </div>
    </header>
  );
}

export function Avatar({ id, large = false }: { id: string; large?: boolean }) {
  const name = agentName(id);
  const letter = id === "human" ? name.charAt(0) : name.charAt(0).toUpperCase();
  const human = id === "human" || /[^\x00-\x7F]/.test(name);
  return (
    <span
      aria-hidden
      className={`c-avatar${large ? " c-avatar--lg" : ""}${human ? " c-avatar--human" : ""}`}
    >
      {letter}
    </span>
  );
}

type Tone = "accent" | "warn" | "error" | "ok" | "muted";

/** Status as a word; color is semantic and muted, never a fill. */
export function StatusWord({ status, label }: { status: string; label?: string }) {
  const map: Record<string, [string, Tone]> = {
    RUNNING: ["進行中", "accent"],
    THINKING: ["思考中", "accent"],
    TOOL_CALL: ["ツール実行中", "accent"],
    COMPLETED: ["完了", "muted"],
    FAILED: ["失敗", "error"],
    BLOCKED: ["保留", "warn"],
    WAITING: ["承認待ち", "warn"],
    OPEN: ["未着手", "muted"],
  };
  const [word, tone] = map[status] ?? [status, "muted"];
  return <span className={tone === "muted" ? "muted" : `tone-${tone}`}>{label ?? word}</span>;
}

export const toMinutes = (hhmm: string) => {
  const [h, m] = hhmm.split(":").map(Number);
  return h * 60 + m;
};

/** Scenario "now" (last workspace update). */
export const NOW = "09:41";

/** Prose with hyphenated identifiers (task-4, edge-02) kept on one line so Japanese wrapping never splits them. */
export function Prose({ text }: { text: string }) {
  return text.split(/([\w()/:.]*\w-[\w().]+)/).map((part, index) =>
    index % 2 ? (
      <span key={index} className="whitespace-nowrap">
        {part}
      </span>
    ) : (
      part
    ),
  );
}

/** Federation URI to short node name (aidash://lab → lab); used across all three screens. */
export const nodeName = (uri: string) => uri.replace("aidash://", "");
