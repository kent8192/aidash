import type { GraphPage } from "./federated-graph";
import type { MeshNode } from "./mesh-model";

export type TimelineEvent = {
  id: string;
  kind: string;
  created_at: string;
  reference: string | null;
};

/** Consume only the current authorized page map; never retain peer history. */
export function mergeGraphTimeline(
  local: readonly TimelineEvent[],
  pages: ReadonlyMap<string, GraphPage>,
  nodes: readonly Pick<MeshNode, "id" | "nodeId">[],
  hours: number,
  now: number,
): TimelineEvent[] {
  const visible = new Map(nodes.map((node) => [node.id, node.nodeId]));
  const events = [...local];
  for (const [peer, page] of pages) {
    if (page.node_id !== peer) continue;
    const references = new Set(
      page.nodes
        .filter((node) => node.node_id === peer && visible.get(node.id) === peer)
        .map((node) => node.id),
    );
    for (const [index, marker] of page.activity.entries()) {
      const time = Date.parse(marker.at);
      if (
        !references.has(marker.reference) ||
        !Number.isFinite(time) ||
        time > now ||
        (hours > 0 && time < now - hours * 3_600_000)
      )
        continue;
      events.push({
        id: JSON.stringify([
          "peer-activity",
          peer,
          marker.reference,
          marker.kind,
          marker.at,
          index,
        ]),
        kind: marker.kind,
        created_at: marker.at,
        reference: marker.reference,
      });
    }
  }
  return events.sort(
    (a, b) =>
      Date.parse(a.created_at) - Date.parse(b.created_at) ||
      a.id.localeCompare(b.id),
  );
}
