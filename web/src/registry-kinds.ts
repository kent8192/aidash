/** Registry kinds in the order people look for them. Unlisted kinds follow
 *  in name order, so a kind the server adds still gets its own group. */
export const registryKindOrder = [
  "agent",
  "model",
  "tool",
  "bundle",
  "skill",
  "memory",
  "source",
  "cluster",
  "decider",
  "embedding",
  "reranker",
  "tokenizer",
  "node",
];

function rank(kind: string): number {
  const index = registryKindOrder.indexOf(kind);
  return index === -1 ? registryKindOrder.length : index;
}

/** Items grouped by kind, groups in `registryKindOrder`, items in input order. */
export function groupByKind<T>(
  items: readonly T[],
  kindOf: (item: T) => string,
): [string, T[]][] {
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const kind = kindOf(item);
    groups.set(kind, [...(groups.get(kind) ?? []), item]);
  }
  return [...groups].sort(
    ([left], [right]) => rank(left) - rank(right) || left.localeCompare(right),
  );
}
