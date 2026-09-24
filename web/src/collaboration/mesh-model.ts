import type { Discovery, MeshEvent, Run, State } from "../types";
import {
  entityKey,
  type EntityReference,
  type Point,
} from "../agent-graph/model.ts";

export const meshKinds = [
  "human",
  "workspace",
  "goal",
  "conversation",
  "task",
  "agent",
  "tool",
  "artifact",
  "cluster",
  "remote",
  "model",
  "skill",
] as const;
export type MeshKind = (typeof meshKinds)[number];
export type MeshMode =
  | "mesh"
  | "collaboration"
  | "knowledge"
  | "execution"
  | "topology";
export type MeshRelation =
  | "contains"
  | "goal"
  | "creates"
  | "assigned"
  | "executes"
  | "delegates"
  | "depends"
  | "produces"
  | "tool"
  | "model"
  | "skill"
  | "member"
  | "hosts"
  | "coordinates"
  | "federation"
  | "participates";
export type MeshNode = {
  id: string;
  kind: MeshKind;
  name: Record<string, string>;
  nodeId: string;
  available: boolean;
  resourceId?: string;
  workspaceId?: string;
  entity?: EntityReference;
  parent?: string;
  status?: string;
  remote?: boolean;
};
export type MeshEdge = {
  id: string;
  source: string;
  target: string;
  relation: MeshRelation;
  layer: "configuration" | "activity" | "federation";
};
export type MeshGraph = {
  nodes: MeshNode[];
  edges: MeshEdge[];
  omitted: number;
  omittedEdges: number;
};
export function canvasEdges(graph: MeshGraph, grouped: boolean): MeshEdge[] {
  if (!grouped) return graph.edges;
  return graph.edges.filter(
    (edge) =>
      edge.relation !== "member" ||
      !graph.nodes.some(
        (node) =>
          (node.id === edge.source && node.parent === edge.target) ||
          (node.id === edge.target && node.parent === edge.source),
      ),
  );
}
export const resourceKey = (nodeId: string, kind: string, id: string) =>
  JSON.stringify(["resource", nodeId, kind, id]);
export function reference(value: unknown): EntityReference | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return;
  const v = value as Record<string, unknown>;
  if (
    typeof v.id === "string" &&
    v.id.trim() &&
    typeof v.version === "string" &&
    v.version.trim()
  )
    return { id: v.id, version: v.version };
}
const activeRun = (run: Run) =>
  !["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
export function inWindow(value: string, hours: number, now: number) {
  const time = Date.parse(value);
  return (
    Number.isFinite(time) &&
    (!hours || (time >= now - hours * 3_600_000 && time <= now))
  );
}

/** An allowlisted projection of authorized snapshots. Never infer relations from prose,
 * arbitrary JSON, knowledge documents, or IDs belonging to another node/version. */
export function buildMeshGraph(
  data: State,
  options: {
    channel?: string;
    discovery?: Discovery;
    runs?: readonly { node: string; run: Run }[];
    hours?: number;
    now?: number;
  } = {},
): MeshGraph {
  const nodeId = data.node.id;
  const now = options.now ?? Date.now();
  const hours = options.hours ?? 0;
  const nodes = new Map<string, MeshNode>();
  const edges = new Map<string, MeshEdge>();
  const add = (node: MeshNode) => {
    if (!nodes.has(node.id)) nodes.set(node.id, node);
    return node.id;
  };
  const edge = (
    source: string,
    target: string,
    relation: MeshRelation,
    layer: MeshEdge["layer"] = "configuration",
  ) => {
    if (source === target) return;
    const id = JSON.stringify([source, relation, target]);
    edges.set(id, { id, source, target, relation, layer });
  };
  const resource = (
    kind: MeshKind,
    id: string,
    name: string,
    workspaceId?: string,
  ) =>
    add({
      id: resourceKey(nodeId, kind, id),
      kind,
      name: { en: name },
      nodeId,
      resourceId: id,
      workspaceId,
      available: true,
    });
  const registryByIdentity = new Map(
    data.registry.map((entry) => [
      JSON.stringify([entry.kind, entry.id, entry.version]),
      entry,
    ]),
  );
  const entryNode = (kind: MeshKind, ref: EntityReference) => {
    const id = entityKey(nodeId, kind, ref);
    const entry = registryByIdentity.get(
      JSON.stringify([kind, ref.id, ref.version]),
    );
    add({
      id,
      kind,
      name: entry?.name ?? {},
      nodeId,
      entity: ref,
      available: Boolean(entry),
    });
    return id;
  };
  const qualified = new Map<string, string>();
  const registry = data.registry.filter((e) =>
    ["agent", "tool", "cluster", "model", "skill"].includes(e.kind),
  );
  for (const e of registry) {
    const id = entryNode(e.kind as MeshKind, { id: e.id, version: e.version });
    if (e.kind === "agent")
      qualified.set(`${nodeId}/agents/${e.id}@${e.version}`, id);
  }
  for (const e of registry) {
    const id = entityKey(nodeId, e.kind, e);
    if (e.kind === "cluster") {
      const ref = reference(e.config?.coordinator);
      if (ref) edge(id, entryNode("agent", ref), "coordinates");
    }
    if (e.kind !== "agent") continue;
    for (const kind of ["tool", "model", "skill", "cluster"] as const) {
      const value =
        e.config?.[kind === "tool" || kind === "skill" ? `${kind}s` : kind];
      for (const item of Array.isArray(value) ? value : [value]) {
        const ref = reference(item);
        if (!ref) continue;
        const target = entryNode(kind, ref);
        edge(id, target, kind === "cluster" ? "member" : kind);
        if (kind === "cluster" && nodes.get(target)?.available)
          nodes.get(id)!.parent = target;
      }
    }
  }
  // Discovery is metadata, not proof of remote activity or health. Subject views
  // use their local authorized State only, even if a discovery cache is present.
  if (data.access.kind === "operator") {
    const local = resource("remote", nodeId, nodeId);
    nodes.get(local)!.status = "LOCAL";
    for (const peer of data.peers.filter((p) => p.enabled)) {
      const id = add({
        id: resourceKey(peer.node_id, "remote", peer.node_id),
        kind: "remote",
        name: { en: peer.node_id },
        nodeId: peer.node_id,
        resourceId: peer.node_id,
        available: true,
        remote: true,
        status: "CONFIGURED",
      });
      edge(local, id, "federation", "federation");
    }
    const peers = new Set(
      data.peers.filter((p) => p.enabled).map((p) => p.node_id),
    );
    for (const { node_id, entity } of options.discovery?.agents ?? []) {
      if (!peers.has(node_id) || entity.kind !== "agent") continue;
      const id = add({
        id: entityKey(node_id, "agent", entity),
        kind: "agent",
        name: entity.name,
        nodeId: node_id,
        entity: { id: entity.id, version: entity.version },
        available: true,
        remote: true,
        status: "DISCOVERED",
      });
      qualified.set(`${node_id}/agents/${entity.id}@${entity.version}`, id);
      edge(resourceKey(node_id, "remote", node_id), id, "hosts", "federation");
    }
    for (const e of registry.filter(
      (e) =>
        e.kind === "cluster" ||
        (e.kind === "agent" && !reference(e.config?.cluster)),
    ))
      edge(local, entityKey(nodeId, e.kind, e), "hosts");
  }
  // Unknown agent principals are not misrepresented as people.
  const principal = (value: string): string | undefined => {
    if (!value) return;
    if (qualified.has(value)) return qualified.get(value);
    if (value.startsWith("aidash://")) return;
    return resource("human", value, value);
  };
  const workspaces = data.workspaces.filter(
    (w) => !options.channel || w.id === options.channel,
  );
  const workspaceIds = new Set(workspaces.map((w) => w.id));
  for (const w of workspaces) {
    const id = resource("workspace", w.id, w.title, w.id);
    if (w.goal.trim()) edge(id, resource("goal", w.id, w.goal, w.id), "goal");
  }
  const tasks = data.tasks.filter((t) => workspaceIds.has(t.workspace_id));
  for (const task of tasks) {
    const id = resource("task", task.id, task.title, task.workspace_id);
    nodes.get(id)!.status = task.status;
    edge(resourceKey(nodeId, "workspace", task.workspace_id), id, "contains");
    const goal = resourceKey(nodeId, "goal", task.workspace_id);
    if (nodes.has(goal) && !task.parent_id) edge(goal, id, "contains");
    if (task.parent_id)
      edge(
        resourceKey(nodeId, "task", task.parent_id),
        id,
        "contains",
        "activity",
      );
    for (const dep of task.dependencies ?? [])
      edge(resourceKey(nodeId, "task", dep), id, "depends", "activity");
    const author = principal(task.created_by);
    const owner = task.owner ? principal(task.owner) : undefined;
    if (author) edge(author, id, "creates", "activity");
    if (owner) edge(id, owner, "assigned", "activity");
    if (
      author &&
      owner &&
      author !== owner &&
      nodes.get(author)?.kind === "agent" &&
      nodes.get(owner)?.kind === "agent"
    )
      edge(author, owner, "delegates", "activity");
  }
  const activity = (
    options.runs ?? data.runs.map((run) => ({ node: nodeId, run }))
  )
    .filter(
      ({ node, run }) =>
        (node === nodeId || data.access.kind === "operator") &&
        (!run.home_node || run.home_node === nodeId) &&
        workspaceIds.has(run.workspace_id) &&
        (activeRun(run) || inWindow(run.updated_at, hours, now)),
    )
    .sort(
      (a, b) =>
        Number(activeRun(b.run)) - Number(activeRun(a.run)) ||
        Date.parse(b.run.updated_at) - Date.parse(a.run.updated_at),
    );
  const statusSet = new Set<string>();
  const enabledPeers = new Set(
    data.peers.filter((peer) => peer.enabled).map((peer) => peer.node_id),
  );
  for (const { node, run } of activity) {
    const identity = `${node}/agents/${run.agent_id}@${run.agent_version}`;
    let id = qualified.get(identity);
    if (!id && node !== nodeId && enabledPeers.has(node)) {
      id = add({
        id: entityKey(node, "agent", {
          id: run.agent_id,
          version: run.agent_version,
        }),
        kind: "agent",
        name: {},
        nodeId: node,
        entity: { id: run.agent_id, version: run.agent_version },
        available: false,
        remote: true,
      });
      qualified.set(identity, id);
      edge(resourceKey(node, "remote", node), id, "hosts", "federation");
    }
    if (!id) continue;
    edge(id, resourceKey(nodeId, "task", run.task_id), "executes", "activity");
    if (!statusSet.has(id)) {
      nodes.get(id)!.status = run.control === "PAUSED" ? "PAUSED" : run.phase;
      statusSet.add(id);
    }
  }
  for (const artifact of data.artifacts.filter((a) =>
    workspaceIds.has(a.workspace_id),
  )) {
    const id = resource(
      "artifact",
      artifact.id,
      artifact.name,
      artifact.workspace_id,
    );
    edge(
      resourceKey(nodeId, "task", artifact.task_id),
      id,
      "produces",
      "activity",
    );
    const author = principal(artifact.created_by);
    if (author) edge(author, id, "produces", "activity");
  }
  for (const c of data.conversations.filter((c) =>
    workspaceIds.has(c.workspace_id),
  )) {
    const workspace = workspaces.find((w) => w.id === c.workspace_id)!;
    const id = resource("conversation", c.id, workspace.title, c.workspace_id);
    edge(resourceKey(nodeId, "workspace", c.workspace_id), id, "contains");
    const author = principal(c.created_by);
    if (author) edge(author, id, "participates", "activity");
    const at = c.target.lastIndexOf("@");
    const target =
      c.target_kind === "agent" && qualified.has(c.target)
        ? qualified.get(c.target)
        : (c.target_kind === "agent" || c.target_kind === "cluster") &&
            at > 0 &&
            at < c.target.length - 1
          ? entityKey(nodeId, c.target_kind, {
              id: c.target.slice(0, at),
              version: c.target.slice(at + 1),
            })
          : undefined;
    if (target && nodes.get(target)?.available)
      edge(target, id, "participates", "activity");
  }
  return {
    nodes: [...nodes.values()],
    edges: [...edges.values()].filter(
      (e) => nodes.has(e.source) && nodes.has(e.target),
    ),
    omitted: 0,
    omittedEdges: 0,
  };
}

export const modeKinds: Record<MeshMode, readonly MeshKind[]> = {
  mesh: meshKinds,
  collaboration: [
    "human",
    "workspace",
    "goal",
    "conversation",
    "agent",
    "cluster",
    "task",
    "artifact",
  ],
  knowledge: [
    "workspace",
    "goal",
    "conversation",
    "task",
    "agent",
    "cluster",
    "artifact",
    "tool",
    "model",
    "skill",
  ],
  execution: ["human", "goal", "task", "agent", "artifact"],
  topology: ["remote", "cluster", "agent", "tool"],
};
export function filterMeshGraph(
  graph: MeshGraph,
  options: {
    mode: MeshMode;
    kinds: readonly MeshKind[];
    query: string;
    relations?: readonly MeshRelation[];
    focus?: string;
    pin?: string;
    maxNodes?: number;
    maxEdges?: number;
  },
): MeshGraph {
  const eligible = graph.nodes.filter(
    (n) =>
      modeKinds[options.mode].includes(n.kind) &&
      options.kinds.includes(n.kind) &&
      (options.mode === "topology" || n.kind !== "remote" || n.remote) &&
      (["mesh", "topology"].includes(options.mode) ||
        !n.remote ||
        graph.edges.some(
          (e) =>
            e.layer === "activity" && (e.source === n.id || e.target === n.id),
        )),
  );
  const query = options.query.trim().toLocaleLowerCase();
  const matches = new Set(
    eligible
      .filter(
        (n) =>
          !query ||
          Object.values(n.name).some((v) =>
            v.toLocaleLowerCase().includes(query),
          ) ||
          n.entity?.id.toLocaleLowerCase().includes(query) ||
          n.resourceId?.toLocaleLowerCase().includes(query),
      )
      .map((n) => n.id),
  );
  const visible = new Set(matches);
  let edges = graph.edges.filter(
    (e) => !options.relations || options.relations.includes(e.relation),
  );
  if (query)
    for (const e of edges)
      if (matches.has(e.source) || matches.has(e.target)) {
        visible.add(e.source);
        visible.add(e.target);
      }
  if (options.focus) {
    const adjacent = new Set([options.focus]);
    for (const e of edges)
      if (e.source === options.focus || e.target === options.focus) {
        adjacent.add(e.source);
        adjacent.add(e.target);
      }
    for (const id of visible) if (!adjacent.has(id)) visible.delete(id);
  }
  const candidates = eligible.filter((n) => visible.has(n.id));
  const candidateCount = candidates.length;
  const maxNodes = options.maxNodes ?? 180;
  // Share a crowded canvas across kinds so registry entries cannot exhaust the
  // budget before any workspace or recorded activity becomes visible.
  if (candidates.length > maxNodes) {
    const priority = [options.focus, options.pin]
      .map((id) => candidates.find((n) => n.id === id))
      .filter((n): n is MeshNode => Boolean(n));
    const groups = new Map<MeshKind, MeshNode[]>();
    for (const candidate of candidates) {
      if (priority.includes(candidate)) continue;
      const group = groups.get(candidate.kind) ?? [];
      group.push(candidate);
      groups.set(candidate.kind, group);
    }
    const ordered = [...new Set(priority)];
    const kinds: MeshKind[] = [
      "workspace",
      "agent",
      "task",
      "artifact",
      "conversation",
      "goal",
      "human",
      "cluster",
      "remote",
      "tool",
      "model",
      "skill",
    ];
    while (ordered.length < maxNodes && groups.size) {
      for (const kind of kinds) {
        const group = groups.get(kind);
        if (!group) continue;
        ordered.push(group.shift()!);
        if (!group.length) groups.delete(kind);
        if (ordered.length === maxNodes) break;
      }
    }
    candidates.splice(0, candidates.length, ...ordered);
  }
  const nodes = candidates.slice(0, maxNodes);
  const ids = new Set(nodes.map((n) => n.id));
  edges = edges.filter((e) => ids.has(e.source) && ids.has(e.target));
  const maxEdges = options.maxEdges ?? 600;
  const keptEdges: MeshEdge[] = [];
  const keptIds = new Set<string>();
  const connected = new Set<string>();
  const keep = (e: MeshEdge) => {
    keptEdges.push(e);
    keptIds.add(e.id);
    connected.add(e.source);
    connected.add(e.target);
  };
  // Cover visible endpoints first, then retain the original relationship order.
  for (const e of edges) {
    if (keptEdges.length >= maxEdges) break;
    if (!connected.has(e.source) && !connected.has(e.target)) keep(e);
  }
  for (const e of edges) {
    if (keptEdges.length >= maxEdges) break;
    if (
      !keptIds.has(e.id) &&
      (!connected.has(e.source) || !connected.has(e.target))
    )
      keep(e);
  }
  for (const e of edges) {
    if (keptEdges.length >= maxEdges) break;
    if (!keptIds.has(e.id)) keep(e);
  }
  return {
    nodes: nodes.map((n) => ({
      ...n,
      parent: n.parent && ids.has(n.parent) ? n.parent : undefined,
    })),
    edges: keptEdges,
    omitted: candidateCount - nodes.length,
    omittedEdges: edges.length - keptEdges.length,
  };
}

/** Semantic lanes reproduce the mesh hierarchy without inventing relationships.
 * Stable IDs, ordering and group dimensions make membership refreshes predictable. */
export function meshPositions(
  graph: MeshGraph,
  mode: MeshMode,
): Record<string, Point> {
  const result: Record<string, Point> = {};
  const compact = () =>
    Object.fromEntries(
      Object.entries(result).map(([id, point]) => [
        id,
        { x: point.x * 0.76, y: point.y * 0.76 },
      ]),
    );
  const nodes = [...graph.nodes].sort((a, b) => a.id.localeCompare(b.id));
  const lane = (
    items: MeshNode[],
    x: number,
    y: number,
    columns: number,
    dx = 170,
    dy = 110,
  ) =>
    items.forEach((n, i) => {
      result[n.id] = {
        x: x + (i % columns) * dx,
        y: y + Math.floor(i / columns) * dy,
      };
    });
  if (mode === "execution") {
    const coordinators = new Set(
      graph.edges
        .filter((edge) => edge.relation === "coordinates")
        .map((edge) => edge.target),
    );
    // The cluster can be filtered out, so recognize explicitly delegating agents too.
    for (const edge of graph.edges)
      if (edge.relation === "delegates") coordinators.add(edge.source);
    lane(
      nodes.filter((n) => n.kind === "human"),
      470,
      30,
      3,
      210,
    );
    lane(
      nodes.filter((n) => n.kind === "agent" && coordinators.has(n.id)),
      470,
      175,
      3,
      210,
    );
    lane(
      nodes.filter((n) => n.kind === "goal"),
      930,
      175,
      1,
    );
    const tasks = nodes.filter((n) => n.kind === "task");
    lane(tasks, 200, 325, 4, 270, 140);
    const agentsY = 325 + Math.max(1, Math.ceil(tasks.length / 4)) * 155;
    const agents = nodes.filter(
      (n) => n.kind === "agent" && !coordinators.has(n.id),
    );
    const taskOrder = (agent: MeshNode) => {
      const related = new Set(
        graph.edges
          .filter(
            (e) =>
              (e.source === agent.id || e.target === agent.id) &&
              ["assigned", "executes"].includes(e.relation),
          )
          .flatMap((e) => [e.source, e.target]),
      );
      const index = tasks.findIndex((task) => related.has(task.id));
      return index < 0 ? tasks.length : index;
    };
    agents.sort((a, b) => taskOrder(a) - taskOrder(b));
    lane(agents, 170, agentsY, 4, 270, 140);
    lane(
      nodes.filter((n) => n.kind === "artifact"),
      270,
      agentsY + Math.max(1, Math.ceil(agents.length / 4)) * 155,
      4,
      220,
    );
    return Object.fromEntries(
      Object.entries(result).map(([id, point]) => [
        id,
        { x: point.x * 0.9, y: point.y * 0.75 },
      ]),
    );
  }
  if (mode === "collaboration") {
    const workspaces = nodes.filter((n) => n.kind === "workspace");
    lane(workspaces, 560, 450, 2, 440, 500);
    const others = nodes.filter((n) => n.kind !== "workspace");
    for (let offset = 0, ring = 0; offset < others.length; ring++) {
      const capacity = 14 + ring * 8;
      const members = others.slice(offset, offset + capacity);
      members.forEach((n, i) => {
        const angle = -Math.PI / 2 + (i / members.length) * Math.PI * 2;
        const radius = 360 + ring * 190;
        result[n.id] = {
          x: 560 + Math.cos(angle) * radius,
          y: 450 + Math.sin(angle) * radius,
        };
      });
      offset += members.length;
    }
    return compact();
  }
  if (mode === "topology") {
    const local = nodes.filter((n) => n.kind === "agent" && !n.remote);
    const groups = [...new Set(local.map((n) => n.parent ?? ""))];
    let nextY = 290;
    for (const parent of groups) {
      const members = local.filter((n) => (n.parent ?? "") === parent);
      lane(members, 320, nextY, 3, 205, 145);
      if (parent) result[parent] = { x: 525, y: nextY };
      nextY += Math.max(1, Math.ceil(members.length / 3)) * 145 + 120;
    }
    lane(
      nodes.filter((n) => n.kind === "cluster" && !groups.includes(n.id)),
      320,
      nextY,
      3,
      205,
    );
    lane(
      nodes.filter((n) => n.kind === "remote" && !n.remote),
      525,
      80,
      2,
      260,
    );
    const peerIds = [
      ...new Set(nodes.filter((n) => n.remote).map((n) => n.nodeId)),
    ];
    peerIds.forEach((nodeId, i) => {
      const x = 1040 + (i % 2) * 290;
      const y = 80 + Math.floor(i / 2) * 520;
      lane(
        nodes.filter((n) => n.nodeId === nodeId && n.kind === "remote"),
        x,
        y,
        1,
      );
      lane(
        nodes.filter((n) => n.nodeId === nodeId && n.kind === "agent"),
        x,
        y + 210,
        1,
        180,
        145,
      );
    });
    lane(
      nodes.filter((n) => n.kind === "tool"),
      450,
      nextY + 10,
      3,
      205,
      145,
    );
    return compact();
  }
  const human = nodes.filter((n) => n.kind === "human");
  lane(human, 580, 20, 4);
  const top = 145 + Math.max(0, Math.ceil(human.length / 4) - 1) * 110;
  lane(
    nodes.filter((n) => n.kind === "workspace"),
    300,
    top,
    1,
  );
  lane(
    nodes.filter((n) => n.kind === "goal"),
    580,
    top,
    3,
    230,
  );
  const taskTop =
    top +
    140 +
    Math.max(
      0,
      Math.ceil(nodes.filter((n) => n.kind === "goal").length / 3) - 1,
    ) *
      110;
  const tasks = nodes.filter((n) => n.kind === "task");
  lane(tasks, 370, taskTop, 4, 215);
  const agentTop =
    taskTop + Math.max(1, Math.ceil(tasks.length / 4)) * 115 + 40;
  const agents = nodes.filter((n) => n.kind === "agent" && !n.remote);
  const parents = [...new Set(agents.map((n) => n.parent ?? ""))];
  let groupY = agentTop;
  for (const parent of parents) {
    const members = agents.filter((n) => (n.parent ?? "") === parent);
    const coordinator = graph.edges.find(
      (e) => e.source === parent && e.relation === "coordinates",
    )?.target;
    members.sort(
      (a, b) => Number(b.id === coordinator) - Number(a.id === coordinator),
    );
    if (coordinator && members.some((n) => n.id === coordinator)) {
      result[coordinator] = { x: 590, y: groupY };
      lane(
        members.filter((n) => n.id !== coordinator),
        365,
        groupY + 125,
        2,
        450,
        145,
      );
    } else lane(members, 365, groupY + 30, 2, 450, 145);
    if (parent) result[parent] = { x: 540, y: groupY };
    groupY +=
      (Math.ceil((members.length - (coordinator ? 1 : 0)) / 2) + 1) * 135;
  }
  lane(
    nodes.filter((n) => n.kind === "cluster" && !parents.includes(n.id)),
    350,
    groupY,
    3,
  );
  lane(
    nodes.filter((n) => n.kind === "tool"),
    100,
    agentTop + 70,
    1,
    150,
    125,
  );
  lane(
    nodes.filter((n) => n.kind === "remote"),
    1110,
    top + 20,
    1,
    160,
    125,
  );
  lane(
    nodes.filter((n) => n.remote && n.kind === "agent"),
    1110,
    agentTop + 55,
    1,
    150,
    115,
  );
  lane(
    nodes.filter((n) => n.kind === "artifact"),
    365,
    Math.max(groupY, agentTop + 150) + 20,
    4,
    215,
  );
  lane(
    nodes.filter((n) => n.kind === "model" || n.kind === "skill"),
    110,
    Math.max(groupY, agentTop + 150) + 60,
    1,
    150,
    100,
  );
  lane(
    nodes.filter((n) => n.kind === "conversation"),
    1050,
    Math.max(groupY, agentTop + 150) + 50,
    1,
  );
  return compact();
}

export function nodeEvents(
  node: MeshNode,
  graph: MeshGraph,
  data: State,
  hours: number,
  now: number,
  channel = "",
): MeshEvent[] {
  const tasks = new Set(
    graph.edges
      .filter((e) => e.source === node.id || e.target === node.id)
      .flatMap((e) => [e.source, e.target])
      .map((id) => graph.nodes.find((n) => n.id === id))
      .filter((n) => n?.kind === "task")
      .map((n) => n!.resourceId),
  );
  if (node.kind === "task") {
    tasks.clear();
    tasks.add(node.resourceId);
  }
  const runIds = new Set(
    data.runs
      .filter(
        (r) =>
          node.nodeId === data.node.id &&
          (!r.home_node || r.home_node === data.node.id) &&
          (!channel || r.workspace_id === channel) &&
          (tasks.has(r.task_id) ||
            (node.kind === "agent" &&
              r.agent_id === node.entity?.id &&
              r.agent_version === node.entity?.version)),
      )
      .map((r) => r.id),
  );
  return data.events
    .filter((e) => {
      if (
        !inWindow(e.created_at, hours, now) ||
        (["remote", "workspace", "goal"].includes(node.kind) &&
          e.node_id !== node.nodeId) ||
        (channel && e.workspace_id !== channel)
      )
        return false;
      if (node.kind === "workspace" || node.kind === "goal")
        return e.workspace_id === node.workspaceId;
      if (node.kind === "remote") return e.node_id === node.nodeId;
      const payload = eventReferences(e);
      return (
        (typeof payload.task_id === "string" && tasks.has(payload.task_id)) ||
        (typeof payload.run_id === "string" && runIds.has(payload.run_id)) ||
        (node.kind === "artifact" && payload.artifact_id === node.resourceId) ||
        (node.kind === "conversation" &&
          payload.conversation_id === node.resourceId)
      );
    })
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
}

/** The server emits full records for creation and compact IDs for run updates.
 * Read only these known envelopes; never search arbitrary event JSON for IDs. */
export function eventReferences(event: MeshEvent): {
  task_id?: string;
  run_id?: string;
  artifact_id?: string;
  conversation_id?: string;
} {
  const record = (value: unknown): Record<string, unknown> =>
    value && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : {};
  const text = (value: unknown) =>
    typeof value === "string" ? value : undefined;
  const data = record(event.data);
  return {
    task_id:
      text(data.task_id) ??
      (event.kind.startsWith("task.")
        ? (text(record(data.task).id) ?? text(data.id))
        : undefined),
    run_id:
      text(data.run_id) ??
      (event.kind.startsWith("run.") ? text(data.id) : undefined),
    artifact_id:
      text(data.artifact_id) ??
      (event.kind === "task.completed"
        ? text(record(data.artifact).id)
        : event.kind.startsWith("artifact.")
          ? text(data.id)
          : undefined),
    conversation_id: event.kind.startsWith("conversation.")
      ? (text(record(data.conversation).id) ?? text(data.id))
      : undefined,
  };
}
