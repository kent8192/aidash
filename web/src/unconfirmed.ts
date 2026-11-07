/**
 * Items a list scanned and could not confirm because the Node that holds the
 * proof did not answer (#770). The count is not a total, and it is not a
 * promise that the items will appear: some can be refused once the Node
 * answers.
 *
 * Written by hand so that a response from a server or peer that predates the
 * field still reads: every reader below treats a missing field as `[]`.
 */
export type UnconfirmedKind =
  | "task"
  | "run"
  | "artifact"
  | "event"
  | "message"
  | "human_request"
  | "semantic_entry"
  | "memory_unit";

export interface Unconfirmed {
  /** The silent Node's address, `aidash://<deployment>/<node name>`. */
  node: string;
  kind: UnconfirmedKind;
  workspace_id: string;
  count: number;
}

/** What a list says about one silent Node. */
export interface UnconfirmedLine {
  node: string;
  count: number;
}

/** The `unconfirmed` of a response; `[]` when the response has none. */
export function readUnconfirmed(body: unknown): Unconfirmed[] {
  const value = (body as { unconfirmed?: unknown } | null | undefined)
    ?.unconfirmed;
  return Array.isArray(value) ? (value as Unconfirmed[]) : [];
}

/**
 * What a list says: for each silent Node, how many rows of `kinds` in the
 * Workspaces in view were left out. Counts of one Node add up, and the Nodes
 * are ordered by address so the lines do not move between refreshes.
 */
export function unconfirmedFor(
  entries: readonly Unconfirmed[] | null | undefined,
  kinds: readonly UnconfirmedKind[],
  workspaceIds: Iterable<string>,
): UnconfirmedLine[] {
  const workspaces = new Set(workspaceIds);
  const counts = new Map<string, number>();
  for (const entry of entries ?? []) {
    if (!kinds.includes(entry.kind) || !workspaces.has(entry.workspace_id))
      continue;
    if (!Number.isFinite(entry.count) || entry.count <= 0) continue;
    counts.set(entry.node, (counts.get(entry.node) ?? 0) + entry.count);
  }
  return [...counts]
    .map(([node, count]) => ({ node, count }))
    .sort((a, b) => (a.node < b.node ? -1 : a.node > b.node ? 1 : 0));
}

/**
 * The name of a Node in a warning: its label, or its address when the
 * Tenant's Node list does not know it. A silent Node must not read as
 * "unavailable item", so this never falls back to a placeholder.
 */
export function unconfirmedNodeName(
  nodes: readonly { address: string; label: string }[],
  address: string,
): string {
  return nodes.find((node) => node.address === address)?.label || address;
}

/** One warning line from the locale's template. `{node}` can occur twice. */
export function unconfirmedText(
  template: string,
  count: number,
  node: string,
): string {
  return template
    .split("{count}")
    .join(String(count))
    .split("{node}")
    .join(node);
}
