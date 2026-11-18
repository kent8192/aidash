/**
 * URL intents: a person pasting an address into a Thread asks the Run to read
 * that page. Page reading is an ordinary opt-in built-in tool, so the call
 * the intent leads to still needs its own Tool call approval.
 */

/** Most pages one input may ask the Run to read; the server enforces it too. */
export const MAX_URL_INTENTS = 5;

/** Longest URL, in UTF-8 bytes, the server accepts as a URL intent. */
export const MAX_URL_BYTES = 4096;

type Reference = { id: string; version: string; registry_deployment?: string };
type Entry = {
  id: string;
  version: string;
  kind: string;
  config?: unknown;
};

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
 * Whether `agent` binds page reading, directly or through a bundle. A tool is
 * page reading when its descriptor names a `research.` provider's `fetch`
 * operation. A bundle Binding exposes only its
 * selected members when it names any, and a nested bundle all of its own,
 * as the server resolves them. `registry` is the entries of the Registry on
 * `node`: a target qualified with another Registry node is never one of them,
 * even when an entry here has the same kind, id and version.
 */
export function bindsPageReading(
  agent: Entry | undefined,
  registry: Entry[],
  node?: string,
): boolean {
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
  return tools.some((tool) => {
    const config = record(tool.config);
    return (
      typeof config?.provider === "string" &&
      config.provider.startsWith("research.") &&
      config.operation === "fetch"
    );
  });
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
