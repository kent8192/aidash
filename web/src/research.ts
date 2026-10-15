/**
 * Web research as the dashboard sees it. Research operations are ordinary
 * capabilities an Agent binds (there is no research mode), so everything here
 * is derived from the Agent's bindings and the Registry entries they name.
 */

/** Most pages one input may ask the Run to read; the server enforces it too. */
export const MAX_URL_INTENTS = 5;

/**
 * One research operation an Agent binds, through a tool or a bundle, with
 * the tool definitions that bind it.
 */
export type ResearchBinding = {
  provider: string;
  operation: string;
  definitions: { id: string; version: string }[];
};

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
    ) {
      const key = `${config.provider} ${config.operation}`;
      const binding = found.get(key) ?? {
        provider: config.provider,
        operation: config.operation,
        definitions: [],
      };
      if (
        !binding.definitions.some(
          (definition) =>
            definition.id === tool.id && definition.version === tool.version,
        )
      )
        binding.definitions.push({ id: tool.id, version: tool.version });
      found.set(key, binding);
    }
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
  // A Page reading grant names the page's host, whichever provider reads it.
  return ja
    ? `この Run でこれ以降の ${host} のページ読み取り（どの Research provider が読む場合も）。新しい入力または期限まで`
    : `Later page readings from ${host} in this Run, whichever Research provider reads them, until new input or expiry`;
}

/**
 * The one line a notification shows for a capability approval: what is
 * asked, where it would go, which Agent asks and for whom. It never carries
 * the disclosed content, which only the card shows, collapsed.
 */
export function approvalNotice(
  card: {
    kind: string;
    operation?: string | null;
    targets: string[];
    agent_id?: string | null;
    agent_version?: string | null;
    requester: string;
  },
  ja: boolean,
): string {
  const what =
    card.kind === "disclosure"
      ? card.operation === "search"
        ? ja
          ? "Web 検索"
          : "Web search"
        : ja
          ? "ページ読み取り"
          : "Page reading"
      : ja
        ? "外部接続"
        : "Outbound request";
  const agent = card.agent_id
    ? `${card.agent_id}@${card.agent_version ?? ""}`
    : null;
  return [
    what,
    card.targets.join(", "),
    agent,
    ja ? `依頼者 ${card.requester}` : `requested by ${card.requester}`,
  ]
    .filter(Boolean)
    .join(" · ");
}

const FAILURES: Record<string, [string, string]> = {
  budget_exhausted: [
    "This Deployment's monthly limit for this Research provider is reached. It is available again next month.",
    "この Deployment のこの Research provider の月間上限に達しました。翌月にまた使えます。",
  ],
  account_unavailable: [
    "This Research provider is unavailable in this Deployment right now.",
    "この Research provider は、この Deployment では現在使えません。",
  ],
  disclosure_denied: [
    "The Web disclosure was denied, expired or closed, so nothing was sent.",
    "Web 開示が拒否・期限切れ・クローズのいずれかになったため、何も送信されませんでした。",
  ],
  non_public_destination: [
    "The page is not on the public Web, so it was not read.",
    "公開 Web 上のページではないため、読み取りませんでした。",
  ],
  evidence_revoked: [
    "This Run's research evidence was revoked, so its research is closed.",
    "この Run の Research evidence が取り消されたため、調査は終了しています。",
  ],
};

/**
 * A research tool result's failure in plain words, or `null` when `result`
 * is not a research failure this dashboard explains. Amounts and budgets
 * are never named.
 */
export function researchFailure(result: unknown, ja: boolean): string | null {
  const value = record(result);
  const error = record(value?.error);
  if (
    value?.version !== 1 ||
    !["search", "fetch", "find"].includes(String(value.operation)) ||
    typeof error?.code !== "string"
  )
    return null;
  const words = FAILURES[error.code];
  return words ? words[ja ? 1 : 0] : null;
}
