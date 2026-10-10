// PROTOTYPE ONLY. Variant A "Signal Desk": keyboard-first operator desk on a calm light surface.
import { useCallback, useEffect, useState } from "react";
import "./a.css";
import { workspaces } from "../data";
import type { Screen, Variant } from "../types";
import { GovernScreen } from "./govern";
import { ObserveScreen } from "./observe";
import { RequestScreen } from "./request";
import type { Decision } from "./request";
import { CommandPalette, StatusLine, TopBar } from "./shell";

const inboxOrder = [
  ...workspaces.filter((w) => w.pending > 0),
  ...workspaces.filter((w) => w.pending === 0 && w.status === "RUNNING"),
  ...workspaces.filter((w) => w.status === "COMPLETED" || w.status === "FAILED"),
].map((w) => w.id);

const hints: Record<Screen, [string, string][]> = {
  request: [
    ["J", "次"],
    ["K", "前"],
    ["A", "承認"],
    ["R", "却下"],
    ["⌘K", "コマンド"],
  ],
  observe: [
    ["F", "絞り込み"],
    ["⌘K", "コマンド"],
  ],
  govern: [
    ["/", "検索"],
    ["⌘K", "コマンド"],
  ],
};

function SignalDesk({ screen, onScreen }: { screen: Screen; onScreen: (s: Screen) => void }) {
  const [palette, setPalette] = useState(false);
  const [decision, setDecision] = useState<Decision>("pending");
  const [selected, setSelected] = useState(inboxOrder[0]);
  const closePalette = useCallback(() => setPalette(false), []);
  const approve = useCallback(() => setDecision("approved"), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
        return;
      }
      if (palette || e.metaKey || e.ctrlKey || e.altKey) return;
      if ((e.target as HTMLElement).closest("input, textarea, select, [contenteditable]")) return;
      const key = e.key.toLowerCase();
      if (key === "1") onScreen("request");
      else if (key === "2") onScreen("observe");
      else if (key === "3") onScreen("govern");
      else if (screen === "request" && key === "a" && decision === "pending") setDecision("approved");
      else if (screen === "request" && key === "r" && decision === "pending") setDecision("rejected");
      else if (screen === "request" && key === "u") setDecision("pending");
      else if (screen === "request" && (key === "j" || key === "k")) {
        const i = inboxOrder.indexOf(selected) + (key === "j" ? 1 : -1);
        setSelected(inboxOrder[Math.min(Math.max(i, 0), inboxOrder.length - 1)]);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, screen, decision, selected, onScreen]);

  return (
    <div className="proto-a flex h-[100dvh] min-h-[100dvh] flex-col overflow-hidden">
      <TopBar screen={screen} onScreen={onScreen} onPalette={() => setPalette(true)} />
      <main className="flex min-h-0 flex-1 flex-col">
        {screen === "request" ? (
          <RequestScreen decision={decision} onDecision={setDecision} selected={selected} onSelect={setSelected} />
        ) : screen === "observe" ? (
          <ObserveScreen />
        ) : (
          <GovernScreen onScreen={onScreen} />
        )}
      </main>
      <StatusLine hints={hints[screen]} />
      {palette ? <CommandPalette onClose={closePalette} onScreen={onScreen} onApprove={approve} /> : null}
    </div>
  );
}

export const variantA: Variant = {
  name: "Signal Desk",
  Component: SignalDesk,
};
