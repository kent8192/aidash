import { entityKey } from "../agent-graph/model";
import { ApiError, apiFetch } from "../transport";
import {
  meshKinds,
  resourceKey,
  type MeshEdge,
  type MeshGraph,
  type MeshKind,
  type MeshMode,
  type MeshNode,
  type MeshRelation,
} from "./mesh-model";

export type GraphPeer = { node_id: string };
export type GraphNode = {
  id: string;
  node_id: string;
  kind: string;
  name: Record<string, string>;
  resource_id: string | null;
  version: string | null;
  workspace_id: string | null;
  status: string | null;
  goal_body: string | null;
  at: string | null;
};
export type GraphActivity = { kind: string; at: string; reference: string };
export type GraphPage = {
  node_id: string;
  generation: string;
  checked_at: string;
  nodes: GraphNode[];
  edges: {
    source: string;
    target: string;
    relation: string;
    layer: string;
  }[];
  activity: GraphActivity[];
  next_cursor: string | null;
};
export type GraphRequest = {
  node_id: string;
  scope_workspace: string | null;
  depth: 1;
  mode: MeshMode;
  kinds: readonly MeshKind[];
  relations: readonly MeshRelation[];
  hours: number;
  limit: number;
  cursor: string | null;
  target_tenant: string | null;
};

export type RemoteExpansion = {
  state:
    | "loading"
    | "ready"
    | "empty"
    | "denied"
    | "unavailable"
    | "unsupported"
    | "oversized";
  page?: GraphPage;
  windowCursor?: string | null;
  checkedAt?: number;
  scope?: string;
};

const projectedKinds = new Set<string>(meshKinds);
const projectedRelations = new Set<MeshRelation>([
  "contains",
  "goal",
  "depends",
  "produces",
  "executes",
  "tool",
  "model",
  "skill",
  "member",
  "hosts",
  "coordinates",
  "participates",
]);
const projectedLayers = new Set(["configuration", "activity", "federation"]);

export async function graphPeers(): Promise<GraphPeer[]> {
  return apiFetch<GraphPeer[]>("/api/federation/graph/peers");
}

export async function graphPage(request: GraphRequest): Promise<GraphPage> {
  return apiFetch<GraphPage>("/api/federation/graph", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request),
  });
}

export function graphError(error: unknown): RemoteExpansion["state"] {
  if (error instanceof ApiError) {
    if (error.status === 401 || error.status === 403) return "denied";
    if (error.status === 404) return "unsupported";
    if (error.status === 400) return "oversized";
  }
  return "unavailable";
}

/** Reject malformed or cross-node projections before they enter search or layout. */
export function validateGraphPage(page: GraphPage, peer: string): boolean {
  if (
    !page ||
    page.node_id !== peer ||
    !Array.isArray(page.nodes) ||
    !Array.isArray(page.edges) ||
    !Array.isArray(page.activity) ||
    page.nodes.length > 80 ||
    page.edges.length > 600 ||
    page.activity.length > 80 ||
    typeof page.generation !== "string" ||
    typeof page.checked_at !== "string" ||
    (page.next_cursor !== null && typeof page.next_cursor !== "string")
  )
    return false;
  const ids = new Set<string>();
  for (const node of page.nodes) {
    if (
      node.node_id !== peer ||
      !projectedKinds.has(node.kind) ||
      node.kind === "remote" ||
      node.kind === "human" ||
      typeof node.resource_id !== "string" ||
      !node.resource_id ||
      !node.name ||
      typeof node.name !== "object" ||
      Array.isArray(node.name) ||
      Object.values(node.name).some((value) => typeof value !== "string")
    )
      return false;
    const expected =
      node.version === null
        ? resourceKey(peer, node.kind, node.resource_id)
        : entityKey(peer, node.kind, {
            id: node.resource_id,
            version: node.version,
          });
    if (
      node.id !== expected ||
      ids.has(node.id) ||
      (node.goal_body !== null &&
        (node.kind !== "goal" || typeof node.goal_body !== "string"))
    )
      return false;
    ids.add(node.id);
  }
  for (const edge of page.edges) {
    if (
      !ids.has(edge.source) ||
      !ids.has(edge.target) ||
      !projectedRelations.has(edge.relation as MeshRelation) ||
      !projectedLayers.has(edge.layer)
    )
      return false;
  }
  for (const marker of page.activity) {
    if (
      !ids.has(marker.reference) ||
      typeof marker.kind !== "string" ||
      marker.kind.length > 100 ||
      !Number.isFinite(Date.parse(marker.at))
    )
      return false;
  }
  return true;
}

export function mergeGraphPages(
  local: MeshGraph,
  localNode: string,
  pages: ReadonlyMap<string, GraphPage>,
  runs: readonly {
    node: string;
    run: {
      id: string;
      task_id: string;
      home_node: string;
      agent_id: string;
      agent_version: string;
    };
  }[],
): MeshGraph {
  const nodes = new Map(local.nodes.map((node) => [node.id, node]));
  const edges = new Map(local.edges.map((edge) => [edge.id, edge]));
  const addEdge = (
    source: string,
    target: string,
    relation: MeshRelation,
    layer: MeshEdge["layer"],
  ) => {
    if (!nodes.has(source) || !nodes.has(target) || source === target) return;
    const id = JSON.stringify([source, relation, target]);
    edges.set(id, { id, source, target, relation, layer });
  };
  for (const [peer, page] of pages) {
    if (!validateGraphPage(page, peer)) continue;
    const boundary = resourceKey(peer, "remote", peer);
    if (!nodes.has(boundary)) continue;
    nodes.set(boundary, { ...nodes.get(boundary)!, projected: true });
    for (const item of page.nodes) {
      const kind = item.kind as MeshKind;
      const node: MeshNode = {
        id: item.id,
        kind,
        name: item.name,
        nodeId: peer,
        available: true,
        resourceId: item.resource_id ?? undefined,
        workspaceId: item.workspace_id ?? undefined,
        entity: item.version
          ? { id: item.resource_id!, version: item.version }
          : undefined,
        status: item.status ?? undefined,
        goalBody: item.goal_body ?? undefined,
        at: item.at ?? undefined,
        remote: true,
        projected: true,
      };
      nodes.set(node.id, node);
      addEdge(boundary, node.id, "hosts", "federation");
    }
    for (const item of page.edges)
      addEdge(
        item.source,
        item.target,
        item.relation as MeshRelation,
        item.layer as MeshEdge["layer"],
      );
    for (const { node, run } of runs) {
      if (node !== peer || run.home_node !== localNode) continue;
      addEdge(
        entityKey(peer, "agent", {
          id: run.agent_id,
          version: run.agent_version,
        }),
        resourceKey(localNode, "run", run.id),
        "executes",
        "activity",
      );
    }
  }
  return { ...local, nodes: [...nodes.values()], edges: [...edges.values()] };
}
