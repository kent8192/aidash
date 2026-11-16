import type { RemoteExpansion } from "./federated-graph";

/** How many rows of one page another Node could not confirm. */
function pageCount(page: RemoteExpansion["page"]): number {
  return (page?.unconfirmed ?? []).reduce((sum, entry) => sum + entry.count, 0);
}

/**
 * Which empty text an empty page shows. A page whose rows were left out
 * because a Node could not confirm them shows none: "no resources" would
 * contradict the warning beside it (as #770 decision 12 does for lists).
 */
export function graphEmptyText(
  page: RemoteExpansion["page"],
): "view" | "page" | null {
  if (pageCount(page) > 0) return null;
  return page?.next_cursor ? "page" : "view";
}

/** Counts belong to the answering Node and only the currently displayed page. */
export function graphUnconfirmed(
  expansions: Readonly<Record<string, RemoteExpansion>>,
  scope: string,
): { node: string; count: number }[] {
  return Object.entries(expansions).flatMap(([node, expansion]) => {
    if (expansion.scope !== scope || !expansion.page) return [];
    const count = pageCount(expansion.page);
    return count > 0 ? [{ node, count }] : [];
  });
}
