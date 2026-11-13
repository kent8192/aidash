import type { RemoteExpansion } from "./federated-graph";

/** Counts belong to the answering Node and only the currently displayed page. */
export function graphUnconfirmed(
  expansions: Readonly<Record<string, RemoteExpansion>>,
  scope: string,
): { node: string; count: number }[] {
  return Object.entries(expansions).flatMap(([node, expansion]) => {
    if (expansion.scope !== scope || !expansion.page) return [];
    const count = (expansion.page.unconfirmed ?? []).reduce(
      (sum, entry) => sum + entry.count,
      0,
    );
    return count > 0 ? [{ node, count }] : [];
  });
}
