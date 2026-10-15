/**
 * Web research as the dashboard sees it. Research operations are ordinary
 * capabilities an Agent binds (there is no research mode), so everything here
 * is derived from the Agent's bindings and the Registry entries they name.
 */

/** Most pages one input may ask the Run to read; the server enforces it too. */
export const MAX_URL_INTENTS = 5;

/** One research operation an Agent binds, through a tool or a bundle. */
export type ResearchBinding = { provider: string; operation: string };

type Reference = { id: string; version: string };
type Entry = {
  id: string;
  version: string;
  kind: string;
  config?: unknown;
};

const ORDER = ["search", "fetch", "find"];

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : undefined;
}

function reference(value: unknown): Reference | undefined {
  const item = record(value);
  return typeof item?.id === "string" && typeof item.version === "string"
    ? { id: item.id, version: item.version }
    : undefined;
}

/**
 * The research operations `agent` binds directly or through a bundle, each
 * once, in contract order. A tool is a research operation when its
 * descriptor names a `research.` provider.
 */
export function researchBindings(
  agent: Entry | undefined,
  registry: Entry[],
): ResearchBinding[] {
  const find = (kind: string, target: Reference | undefined) =>
    target &&
    registry.find(
      (entry) =>
        entry.kind === kind &&
        entry.id === target.id &&
        entry.version === target.version,
    );
  const tools: Entry[] = [];
  const bindings = record(agent?.config)?.bindings;
  for (const binding of Array.isArray(bindings) ? bindings : []) {
    const item = record(binding);
    const target = reference(item?.target);
    if (item?.kind === "tool") {
      const tool = find("tool", target);
      if (tool) tools.push(tool);
    } else if (item?.kind === "bundle") {
      const members = record(find("bundle", target)?.config)?.members;
      for (const member of Array.isArray(members) ? members : []) {
        const tool = find("tool", reference(member));
        if (tool) tools.push(tool);
      }
    }
  }
  const found = new Map<string, ResearchBinding>();
  for (const tool of tools) {
    const config = record(tool.config);
    if (
      typeof config?.provider === "string" &&
      config.provider.startsWith("research.") &&
      typeof config.operation === "string"
    )
      found.set(`${config.provider} ${config.operation}`, {
        provider: config.provider,
        operation: config.operation,
      });
  }
  return [...found.values()].sort(
    (a, b) =>
      ORDER.indexOf(a.operation) - ORDER.indexOf(b.operation) ||
      a.provider.localeCompare(b.provider),
  );
}

/**
 * Distinct absolute http(s) URLs in `text`, in order of appearance, that the
 * host would accept as URL intents: no credentials and no fragment. Trailing
 * sentence punctuation is not part of a URL.
 */
export function urlIntents(text: string): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(/https?:\/\/[^\s<>"'`]+/gi)) {
    const candidate = match[0].replace(/[.,;:!?)\]}]+$/u, "");
    let url: URL;
    try {
      url = new URL(candidate);
    } catch {
      continue;
    }
    if (
      !["http:", "https:"].includes(url.protocol) ||
      url.username ||
      url.password ||
      candidate.includes("#")
    )
      continue;
    if (!found.includes(candidate)) found.push(candidate);
  }
  return found;
}

/** A recorded price in micro-USD as dollars, e.g. `$0.0050`. */
export function dollars(microUsd: number): string {
  return `$${(microUsd / 1_000_000).toFixed(4)}`;
}

/**
 * What an "allow for this Run" decision on a Web disclosure covers. A Run
 * grant names a destination host, so for search it covers every later query
 * through that provider, including queries the approver has not seen.
 */
export function runGrantScope(
  card: {
    operation?: string | null;
    provider?: string | null;
    targets: string[];
  },
  ja: boolean,
): string {
  const host = card.targets[0] ?? "";
  if (card.operation === "search") {
    const provider = card.provider ?? host;
    return ja
      ? `この Run でこれ以降の ${provider} 検索すべて（まだ見ていない検索語を含む）。新しい入力または期限まで`
      : `Every later ${provider} search in this Run, including queries you have not seen, until new input or expiry`;
  }
  return ja
    ? `この Run でこれ以降の ${host} のページ読み取り。新しい入力または期限まで`
    : `Later page readings from ${host} in this Run, until new input or expiry`;
}
