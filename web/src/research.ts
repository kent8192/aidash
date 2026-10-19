/**
 * Web research as the dashboard sees it. Research operations are ordinary
 * capabilities an Agent binds (there is no research mode), so everything here
 * is derived from the Agent's bindings and the Registry entries they name.
 */

/** Most pages one input may ask the Run to read; the server enforces it too. */
export const MAX_URL_INTENTS = 5;

/** Longest URL, in UTF-8 bytes, the server accepts as a URL intent. */
export const MAX_URL_BYTES = 4096;

/**
 * One research operation an Agent binds, through a tool or a bundle, with
 * the tool definitions that bind it.
 */
export type ResearchBinding = {
  provider: string;
  operation: string;
  definitions: { id: string; version: string }[];
};

type Reference = { id: string; version: string; registry_deployment?: string };
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
    ? {
        id: item.id,
        version: item.version,
        registry_deployment:
          typeof item.registry_deployment === "string"
            ? item.registry_deployment
            : undefined,
      }
    : undefined;
}

/**
 * The research operations `agent` binds directly or through a bundle, each
 * once, in contract order. A tool is a research operation when its
 * descriptor names a `research.` provider. A bundle Binding exposes only its
 * selected members when it names any, and a nested bundle all of its own,
 * as the server resolves them. `registry` is the entries of the Registry on
 * `node`: a target qualified with another Registry node is never one of them,
 * even when an entry here has the same kind, id and version.
 */
export function researchBindings(
  agent: Entry | undefined,
  registry: Entry[],
  node?: string,
): ResearchBinding[] {
  const find = (kind: string, target: Reference | undefined) =>
    target &&
    (node === undefined ||
      target.registry_deployment === undefined ||
      target.registry_deployment === node) &&
    registry.find(
      (entry) =>
        entry.kind === kind &&
        entry.id === target.id &&
        entry.version === target.version,
    );
  const tools: Entry[] = [];
  const expand = (
    bundle: Entry | false | undefined,
    selected: string[],
    seen: Set<string>,
  ) => {
    // The server refuses a recursive bundle; here it simply ends.
    if (!bundle || seen.has(`${bundle.id}@${bundle.version}`)) return;
    seen.add(`${bundle.id}@${bundle.version}`);
    const members = record(bundle.config)?.members;
    for (const member of Array.isArray(members) ? members : []) {
      const target = reference(member);
      if (!target || (selected.length > 0 && !selected.includes(target.id)))
        continue;
      const tool = find("tool", target);
      if (tool) tools.push(tool);
      else expand(find("bundle", target), [], seen);
    }
  };
  const bindings = record(agent?.config)?.bindings;
  for (const binding of Array.isArray(bindings) ? bindings : []) {
    const item = record(binding);
    const target = reference(item?.target);
    if (item?.kind === "tool") {
      const tool = find("tool", target);
      if (tool) tools.push(tool);
    } else if (item?.kind === "bundle") {
      const selected = Array.isArray(item.members)
        ? item.members.filter(
            (member): member is string => typeof member === "string",
          )
        : [];
      expand(find("bundle", target), selected, new Set());
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
 * Whether a Run's binding snapshot reads pages, as the server decides before
 * it records URL intents for the Run: a tool Binding the Run was admitted
 * with, not excluded then, whose descriptor names a `research.` provider's
 * `fetch` operation. A later Catalog Approval does not change the snapshot,
 * and a remote Run never reads pages.
 */
export function snapshotReadsPages(snapshot: unknown): boolean {
  const item = record(snapshot);
  if (!item || item.remote !== false || !Array.isArray(item.bindings))
    return false;
  return item.bindings.some((binding) => {
    const resolved = record(binding);
    const definition = record(resolved?.definition);
    const config = record(definition?.config);
    return (
      resolved?.excluded_reason == null &&
      definition?.kind === "tool" &&
      typeof config?.provider === "string" &&
      config.provider.startsWith("research.") &&
      config.operation === "fetch"
    );
  });
}

const CLOSING: Record<string, string> = { ")": "(", "]": "[", "}": "{" };

/**
 * `raw` without trailing sentence punctuation, or a closing bracket the URL
 * does not open itself: `https://en.wikipedia.org/wiki/Foo_(bar)` keeps its
 * parenthesis, while `(https://example.net/c)` loses the wrapper's.
 */
function trimUrl(raw: string): string {
  let url = raw;
  for (;;) {
    const last = url.at(-1) ?? "";
    const opening = CLOSING[last];
    const count = (character: string) => url.split(character).length - 1;
    if (".,;:!?".includes(last) && last !== "") url = url.slice(0, -1);
    else if (opening && count(last) > count(opening)) url = url.slice(0, -1);
    else return url;
  }
}

/**
 * Distinct absolute http(s) URLs in `text`, in order of appearance, that the
 * host would accept as URL intents: no credentials, no fragment, no control
 * character and at most `MAX_URL_BYTES` long. Trailing sentence punctuation is not part of a URL,
 * unless the URL is written in angle brackets (`<https://example.com/Yahoo!>`,
 * RFC 3986 Appendix C), which keep it exactly as written.
 */
// The characters Rust's `char::is_control` matches (Unicode category Cc).
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

export function urlIntents(text: string): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(/https?:\/\/[^\s<>"'`]+/gi)) {
    const start = match.index ?? 0;
    const delimited =
      text[start - 1] === "<" && text[start + match[0].length] === ">";
    const candidate = delimited ? match[0] : trimUrl(match[0]);
    if (new TextEncoder().encode(candidate).length > MAX_URL_BYTES) continue;
    // The host refuses a URL with any control character, which `new URL`
    // would accept by percent-encoding it.
    if (CONTROL.test(candidate)) continue;
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

/** A recorded price in micro-USD as dollars, e.g. `$0.0050`: at least four
 *  decimals and as many more, up to six, as the amount needs, so a nonzero
 *  price never reads as zero. */
export function dollars(microUsd: number): string {
  return `$${(microUsd / 1_000_000).toFixed(6).replace(/(\.\d{4}\d*?)0+$/, "$1")}`;
}

/** One request's price as an approver reads it. A zero price is a free
 *  request that is still attributed to the Research account. */
export function requestPrice(microUsd: number, ja: boolean): string {
  if (microUsd === 0) return ja ? "無料" : "Free";
  return `${dollars(microUsd)} ${ja ? "（見積もり。請求額ではありません）" : "(estimate, not an invoice)"}`;
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
  result_expired: [
    "The research service's account terms no longer let this result be kept. The agent may search or read again.",
    "調査サービスの利用規約 (Research account terms) により、この結果の保持期限が切れました。エージェントは再度検索または読み取りできます。",
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

/** The periods of Retention Rules, in seconds (ADR 0045). */
export type Periods = {
  document_snapshots: { after_run_seconds: number; max_age_seconds: number };
};

/** The units a retention period is entered in, longest first. */
export const PERIOD_UNITS = [
  ["days", 86_400],
  ["hours", 3_600],
  ["minutes", 60],
  ["seconds", 1],
] as const;

export type PeriodUnit = (typeof PERIOD_UNITS)[number][0];

const UNIT_WORDS: Record<PeriodUnit, [string, string, string]> = {
  days: ["day", "days", "日"],
  hours: ["hour", "hours", "時間"],
  minutes: ["minute", "minutes", "分"],
  seconds: ["second", "seconds", "秒"],
};

/**
 * `seconds` in the longest unit that divides it evenly, so a period reads
 * as it was meant ("7 days", not "168 hours"). Zero reads in hours.
 */
export function splitPeriod(seconds: number): {
  value: number;
  unit: PeriodUnit;
} {
  if (seconds === 0) return { value: 0, unit: "hours" };
  const [unit, size] =
    PERIOD_UNITS.find(([, size]) => seconds % size === 0) ??
    PERIOD_UNITS[PERIOD_UNITS.length - 1];
  return { value: seconds / size, unit };
}

/** A period entered as a whole number of `unit`s, in seconds. */
export function periodSeconds(value: number, unit: PeriodUnit): number {
  return value * PERIOD_UNITS.find(([name]) => name === unit)![1];
}

/** A period in words, such as "24 hours" or "7 日". */
export function periodWords(seconds: number, ja: boolean): string {
  const { value, unit } = splitPeriod(seconds);
  const [one, many, japanese] = UNIT_WORDS[unit];
  return ja ? `${value} ${japanese}` : `${value} ${value === 1 ? one : many}`;
}

/** Which periods `next` shortens compared with `current`. */
export function shortenedPeriods(
  current: Periods,
  next: Periods,
): ("after_run_seconds" | "max_age_seconds")[] {
  return (["after_run_seconds", "max_age_seconds"] as const).filter(
    (period) =>
      next.document_snapshots[period] < current.document_snapshots[period],
  );
}

/** Whether a Tenant's stored period exceeds the Deployment's current one. */
export function exceedsCeiling(tenant: Periods | null, deployment: Periods) {
  return (
    !!tenant &&
    (["after_run_seconds", "max_age_seconds"] as const).some(
      (period) =>
        tenant.document_snapshots[period] >
        deployment.document_snapshots[period],
    )
  );
}

/**
 * Who revoked evidence or changed Retention Rules, as recorded:
 * `subject:<tenant>/<subject>` shows the subject. A Deployment
 * Administrator's identity is shown only to Deployment Administrators.
 */
export function recordedActor(
  recorded: string,
  deploymentAdministrator: boolean,
  ja: boolean,
): string {
  const administrator = ja
    ? "デプロイメント管理者"
    : "Deployment Administrator";
  if (recorded.startsWith("subject:"))
    return recorded.slice(recorded.indexOf("/") + 1) || recorded;
  if (recorded.startsWith("deployment-administrator:"))
    return deploymentAdministrator
      ? `${administrator} (${recorded.slice("deployment-administrator:".length)})`
      : administrator;
  if (recorded === "deployment-administrator")
    return deploymentAdministrator
      ? `${administrator} (${ja ? "API トークン" : "API token"})`
      : administrator;
  return recorded;
}

/** Longest revocation reason the server accepts, in characters. */
export const MAX_REVOCATION_REASON = 1000;

/** Whether a Run has done any research a revocation would remove. */
export function hasResearch(research: {
  usage: { search_attempts: number; fetch_attempts: number; sources: number };
  uncertain_attempts: number;
  in_flight_attempts: number;
}): boolean {
  return (
    research.usage.search_attempts +
      research.usage.fetch_attempts +
      research.usage.sources +
      research.uncertain_attempts +
      research.in_flight_attempts >
    0
  );
}
