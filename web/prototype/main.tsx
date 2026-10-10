// PROTOTYPE ONLY. Three redesign variants × three screens, switchable via ?variant=&screen=.
// Run: `npm run prototype` (from web/). Not part of the production build.
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import "./prototype.css";
import { variantA } from "./variant-a";
import { variantB } from "./variant-b";
import { variantC } from "./variant-c";
import type { Screen, Variant } from "./types";

const variants: Record<string, Variant> = { A: variantA, B: variantB, C: variantC };
const keys = Object.keys(variants);
const screens: { key: Screen; label: string }[] = [
  { key: "request", label: "依頼" },
  { key: "observe", label: "観測" },
  { key: "govern", label: "統制" },
];

function readLocation() {
  const params = new URLSearchParams(location.search);
  const variant = keys.includes(params.get("variant") ?? "") ? params.get("variant")! : "A";
  const screen = screens.some((s) => s.key === params.get("screen"))
    ? (params.get("screen") as Screen)
    : "request";
  return { variant, screen };
}

function App() {
  const [state, setState] = useState(readLocation);
  const update = (next: Partial<typeof state>) => {
    const merged = { ...state, ...next };
    history.replaceState(null, "", `?variant=${merged.variant}&screen=${merged.screen}`);
    setState(merged);
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (target.closest("input, textarea, select, [contenteditable]")) return;
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      const step = event.key === "ArrowLeft" ? -1 : 1;
      const index = (keys.indexOf(state.variant) + step + keys.length) % keys.length;
      update({ variant: keys[index] });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const { Component, name } = variants[state.variant];
  return (
    <>
      <Component screen={state.screen} onScreen={(screen) => update({ screen })} />
      <nav className="proto-switcher" aria-label="Prototype switcher">
        {keys.map((key) => (
          <button
            key={key}
            type="button"
            aria-pressed={key === state.variant}
            onClick={() => update({ variant: key })}
          >
            {key === state.variant ? `${key} · ${name}` : key}
          </button>
        ))}
        <span className="sep" />
        {screens.map((s) => (
          <button
            key={s.key}
            type="button"
            aria-pressed={s.key === state.screen}
            onClick={() => update({ screen: s.key })}
          >
            {s.label}
          </button>
        ))}
      </nav>
    </>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
