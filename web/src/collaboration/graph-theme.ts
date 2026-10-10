import type { Core, StylesheetJson } from "cytoscape";

/** Resolved Night Ops tokens in a form canvas renderers accept. */
export type GraphTokens = {
  background: string;
  surface: string;
  raised: string;
  border: string;
  borderStrong: string;
  foreground: string;
  muted: string;
  faint: string;
  brand: string;
  brandMark: string;
  warning: string;
  destructive: string;
  success: string;
  edge: string;
  edgeStrong: string;
  sans: string;
  mono: string;
};

let probe: CanvasRenderingContext2D | null | undefined;

/** Canvas colour parsing normalises any CSS colour (including `rgb(r g b / a)`) to opaque hex; graph tokens are opaque. */
function opaque(value: string): string {
  probe ??= document.createElement("canvas").getContext("2d");
  if (!probe || !value) return value;
  probe.fillStyle = "#000000";
  probe.fillStyle = value;
  const normalised = String(probe.fillStyle);
  const rgba = /^rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(normalised);
  if (!rgba) return normalised;
  return `#${rgba
    .slice(1, 4)
    .map((channel) => Number(channel).toString(16).padStart(2, "0"))
    .join("")}`;
}

export function graphTokens(): GraphTokens {
  const css = getComputedStyle(document.documentElement);
  const token = (name: string) => css.getPropertyValue(name).trim();
  const color = (name: string) => opaque(token(name));
  return {
    background: color("--background"),
    surface: color("--surface"),
    raised: color("--raised"),
    border: color("--border"),
    borderStrong: color("--border-strong"),
    foreground: color("--foreground"),
    muted: color("--muted-foreground"),
    faint: color("--faint"),
    brand: color("--brand"),
    brandMark: color("--brand-mark"),
    warning: color("--warning"),
    destructive: color("--destructive"),
    success: color("--success"),
    edge: color("--edge"),
    edgeStrong: color("--edge-strong"),
    sans:
      token("--font-sans") ||
      '"IBM Plex Sans", "IBM Plex Sans JP", ui-sans-serif, sans-serif',
    mono: token("--font-mono") || '"IBM Plex Mono", ui-monospace, monospace',
  };
}

/** Calls `apply` whenever the resolved theme on <html> changes. */
export function onThemeChange(apply: () => void): () => void {
  const observer = new MutationObserver(apply);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
  return () => observer.disconnect();
}

/**
 * Keeps a Cytoscape instance on the current theme. With `stylesheet`, the whole
 * style is rebuilt from tokens; without it, only the shared node/edge colours
 * of a caller-defined style are patched.
 */
export function observeGraphTheme(
  cy: Core,
  stylesheet?: (tokens: GraphTokens) => StylesheetJson,
): () => void {
  const apply = () => {
    const tokens = graphTokens();
    if (stylesheet) {
      cy.style(stylesheet(tokens));
      return;
    }
    cy.style()
      .selector("node")
      .style({
        "background-color": tokens.surface,
        "border-color": tokens.borderStrong,
        color: tokens.muted,
        "font-family": tokens.sans,
      })
      .selector("edge")
      .style({
        "line-color": tokens.edge,
        "target-arrow-color": tokens.edge,
        color: tokens.muted,
        "text-background-color": tokens.background,
      })
      .update();
  };
  apply();
  return onThemeChange(apply);
}
