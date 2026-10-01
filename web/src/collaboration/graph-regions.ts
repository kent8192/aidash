import type { State } from "../types";
import type { Point } from "../agent-graph/model";
import { resourceKey, type MeshGraph, type MeshNode } from "./mesh-model.ts";

export type GraphRegion = {
  id: string;
  kind: "execution" | "shared" | "configuration";
  nodeId: string;
  workspaceId?: string;
  workspaceName?: Record<string, string>;
};

const sharedKinds = new Set([
  "workspace",
  "goal",
  "task",
  "conversation",
  "artifact",
]);
const configurationKinds = new Set(["tool", "model", "skill", "cluster"]);

export function regionForNode(node: MeshNode): GraphRegion | undefined {
  if (node.kind === "agent" || node.kind === "run")
    return {
      id: JSON.stringify(["region", "execution", node.nodeId]),
      kind: "execution",
      nodeId: node.nodeId,
    };
  if (sharedKinds.has(node.kind) && node.workspaceId)
    return {
      id: JSON.stringify(["region", "shared", node.nodeId, node.workspaceId]),
      kind: "shared",
      nodeId: node.nodeId,
      workspaceId: node.workspaceId,
    };
  if (configurationKinds.has(node.kind))
    return {
      id: JSON.stringify(["region", "configuration", node.nodeId]),
      kind: "configuration",
      nodeId: node.nodeId,
    };
}

export function graphRegions(
  graph: MeshGraph,
  localNode: string,
): GraphRegion[] {
  const regions = new Map<string, GraphRegion>();
  for (const node of graph.nodes) {
    const region = regionForNode(node);
    if (region) regions.set(region.id, region);
  }
  return [...regions.values()]
    .map((region) => ({
      ...region,
      workspaceName: graph.nodes.find(
        (node) =>
          region.kind === "shared" &&
          node.kind === "workspace" &&
          node.nodeId === region.nodeId &&
          node.resourceId === region.workspaceId,
      )?.name,
    }))
    .sort((a, b) => {
      const rank = { execution: 0, shared: 1, configuration: 2 };
      return (
        rank[a.kind] - rank[b.kind] ||
        Number(b.nodeId === localNode) - Number(a.nodeId === localNode) ||
        a.nodeId.localeCompare(b.nodeId) ||
        (a.workspaceId ?? "").localeCompare(b.workspaceId ?? "")
      );
    });
}

/** Scope by recorded, authorized resources before applying perspective and UI filters. */
export function workspaceGraph(
  graph: MeshGraph,
  localNode: string,
  workspace: string,
  state: Pick<State, "tasks" | "conversations">,
): MeshGraph {
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  const keep = new Set<string>();
  const workspaceIds = new Set(
    graph.nodes
      .filter((node) => node.kind === "workspace" && node.resourceId)
      .filter(
        (node) =>
          !workspace ||
          (node.nodeId === localNode && node.resourceId === workspace),
      )
      .map((node) => JSON.stringify([node.nodeId, node.resourceId])),
  );
  const selectedWorkspace = workspace
    ? JSON.stringify([localNode, workspace])
    : "";
  for (const node of graph.nodes) {
    if (!node.workspaceId) continue;
    const owner = JSON.stringify([node.nodeId, node.workspaceId]);
    // Authorized continuation pages need not repeat the Workspace vertex.
    if (sharedKinds.has(node.kind) && (!workspace || workspaceIds.has(owner)))
      keep.add(node.id);
    if (
      node.kind === "run" &&
      (!workspace ||
        (workspaceIds.has(selectedWorkspace) && node.workspaceId === workspace))
    )
      keep.add(node.id);
  }
  const operational = new Set(keep);
  for (const edge of graph.edges) {
    if (
      !["creates", "assigned", "executes", "participates"].includes(
        edge.relation,
      )
    )
      continue;
    const source = byId.get(edge.source);
    const target = byId.get(edge.target);
    if (source?.kind === "agent" && operational.has(edge.target))
      keep.add(source.id);
    if (target?.kind === "agent" && operational.has(edge.source))
      keep.add(target.id);
    if (source?.kind === "human" && operational.has(edge.target))
      keep.add(source.id);
    if (target?.kind === "human" && operational.has(edge.source))
      keep.add(target.id);
  }
  // A Home Task can name an exact remote Agent even when the old graph builder
  // cannot make a local Registry edge to that Peer. The Agent must still be in
  // the current authorized projection before it can be displayed.
  const principals = [
    ...state.tasks
      .filter((task) => keep.has(resourceKey(localNode, "task", task.id)))
      .flatMap((task) => [task.created_by, task.owner]),
    ...state.conversations
      .filter((conversation) =>
        keep.has(resourceKey(localNode, "conversation", conversation.id)),
      )
      .flatMap((conversation) => [
        conversation.created_by,
        conversation.target,
      ]),
  ];
  const exact = new Set(principals.filter((value): value is string => !!value));
  for (const node of graph.nodes) {
    if (node.kind !== "agent" || !node.entity || keep.has(node.id)) continue;
    const principal = `${node.nodeId}/agents/${node.entity.id}@${node.entity.version}`;
    if (exact.has(principal)) keep.add(node.id);
  }
  for (const edge of graph.edges) {
    if (!keep.has(edge.source)) continue;
    if (!["tool", "model", "skill", "member"].includes(edge.relation)) continue;
    if (
      byId.get(edge.source)?.kind === "agent" &&
      configurationKinds.has(byId.get(edge.target)?.kind ?? "")
    )
      keep.add(edge.target);
  }
  return {
    ...graph,
    nodes: graph.nodes.filter((node) => keep.has(node.id)),
    edges: graph.edges.filter(
      (edge) => keep.has(edge.source) && keep.has(edge.target),
    ),
  };
}

/** Stable lanes for compound regions; Cytoscape owns subsequent drag and zoom. */
export function graphRegionPositions(
  graph: MeshGraph,
  localNode: string,
): Record<string, Point> {
  const positions: Record<string, Point> = {};
  const regions = graphRegions(graph, localNode);
  let y = 90;
  for (const kind of ["execution", "shared", "configuration"] as const) {
    const row = regions.filter((region) => region.kind === kind);
    for (let offset = 0; offset < row.length; offset += 2) {
      const pair = row.slice(offset, offset + 2);
      let height = 0;
      pair.forEach((region, column) => {
        const members = graph.nodes
          .filter((node) => regionForNode(node)?.id === region.id)
          .sort((a, b) => {
            const order =
              kind === "execution"
                ? ["agent", "run"]
                : kind === "shared"
                  ? ["workspace", "goal", "task", "conversation", "artifact"]
                  : ["cluster", "tool", "model", "skill"];
            return (
              order.indexOf(a.kind) - order.indexOf(b.kind) ||
              a.id.localeCompare(b.id)
            );
          });
        const columns = kind === "shared" ? 3 : 2;
        const rows = Math.ceil(members.length / columns);
        const dx = kind === "shared" ? 175 : 205;
        members.forEach((node, index) => {
          positions[node.id] = {
            x: 270 + column * 660 + (index % columns) * dx,
            y: y + 75 + Math.floor(index / columns) * 125,
          };
        });
        height = Math.max(height, Math.max(rows, 1) * 125 + 120);
      });
      y += height;
    }
    if (row.length) y += 95;
  }
  graph.nodes
    .filter((node) => node.kind === "human")
    .sort((a, b) => a.id.localeCompare(b.id))
    .forEach((node, index) => {
      positions[node.id] = { x: 80, y: 90 + index * 125 };
    });
  return positions;
}
