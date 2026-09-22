import type { Entry, Run, Task } from "../types";

export const graphKinds = ["agent", "model", "tool", "skill", "cluster", "run", "task"] as const;
export type GraphKind = (typeof graphKinds)[number];
export type EntityKind = Exclude<GraphKind, "run" | "task">;
export type EntityReference = { id: string; version: string };
export type Relationship = "model" | "tool" | "skill" | "cluster" | "coordinator" | "run" | "executes";
export type GraphNode = {
  id: string;
  kind: GraphKind;
  name: Record<string, string>;
  available: boolean;
  entity?: EntityReference;
  resourceId?: string;
  status?: string;
};
export type GraphEdge = {
  id: string;
  source: string;
  target: string;
  relation: Relationship;
  layer: "configuration" | "runtime";
};
export type AgentGraph = { rootId: string; nodes: GraphNode[]; edges: GraphEdge[]; omitted: number; omittedEdges: number };
export type GraphInput = {
  nodeId: string;
  root: EntityReference;
  entries: readonly Pick<Entry, "id" | "version" | "kind" | "name" | "config">[];
  runs: readonly Pick<Run, "id" | "agent_id" | "agent_version" | "task_id" | "phase" | "control">[];
  tasks: readonly Pick<Task, "id" | "title" | "status">[];
};
export type GraphOptions = {
  expandedIds?: readonly string[];
  kinds?: readonly GraphKind[];
  runtime?: boolean;
  maxNodes?: number;
};
export type Point = { x: number; y: number };
export type Positions = Record<string, Point>;

export function entityKey(nodeId: string, kind: string, reference: EntityReference): string {
  return JSON.stringify(["entity", nodeId, kind, reference.id, reference.version]);
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function reference(value: unknown): EntityReference | undefined {
  const r = record(value);
  if (typeof r.id === "string" && r.id.trim() && typeof r.version === "string" && r.version.trim()) {
    return { id: r.id, version: r.version };
  }
}

function localized(value: unknown): Record<string, string> {
  return Object.fromEntries(Object.entries(record(value)).filter((pair): pair is [string, string] => typeof pair[1] === "string"));
}

/** Project only explicit relationships in the caller's authorized, node-local State.
 * Deliberately do not traverse arbitrary config, descriptions, knowledge or discovery.
 */
export function buildAgentGraph(input: GraphInput, options: GraphOptions = {}): AgentGraph | null {
  const rootId = entityKey(input.nodeId, "agent", input.root);
  const entries = new Map(input.entries
    .filter((entry) => graphKinds.slice(0, 5).includes(entry.kind as EntityKind))
    .map((entry) => [entityKey(input.nodeId, entry.kind, entry), entry]));
  if (!entries.has(rootId)) return null;

  const nodes = new Map<string, GraphNode>();
  const edges = new Map<string, GraphEdge>();
  const addEntity = (kind: EntityKind, ref: EntityReference): string => {
    const id = entityKey(input.nodeId, kind, ref);
    if (!nodes.has(id)) {
      const entry = entries.get(id);
      nodes.set(id, { id, kind, entity: { id: ref.id, version: ref.version }, name: entry ? localized(entry.name) : {}, available: Boolean(entry) });
    }
    return id;
  };
  const connect = (source: string, target: string, relation: Relationship, layer: GraphEdge["layer"]) => {
    const id = JSON.stringify([source, relation, target]);
    edges.set(id, { id, source, target, relation, layer });
  };
  const configure = (source: string, kind: EntityKind, value: unknown, relation: Relationship) => {
    const ref = reference(value);
    if (ref) connect(source, addEntity(kind, ref), relation, "configuration");
  };
  for (const entry of entries.values()) {
    const source = addEntity(entry.kind as EntityKind, entry);
    const config = record(entry.config);
    if (entry.kind === "agent") {
      configure(source, "model", config.model, "model");
      configure(source, "cluster", config.cluster, "cluster");
      for (const kind of ["tool", "skill"] as const) {
        const values = config[`${kind}s`];
        if (Array.isArray(values)) {
          for (const value of values) configure(source, kind, value, kind);
        }
      }
    } else if (entry.kind === "cluster") {
      configure(source, "agent", config.coordinator, "coordinator");
    }
  }
  if (options.runtime !== false) {
    const tasks = new Map(input.tasks.map((task) => [task.id, task]));
    for (const run of input.runs) {
      const agentId = entityKey(input.nodeId, "agent", { id: run.agent_id, version: run.agent_version });
      if (!entries.has(agentId)) continue;
      const id = JSON.stringify(["run", input.nodeId, run.id]);
      const taskId = JSON.stringify(["task", input.nodeId, run.task_id]);
      const task = tasks.get(run.task_id);
      nodes.set(id, { id, kind: "run", name: {}, resourceId: run.id, status: run.control === "PAUSED" ? "PAUSED" : run.phase, available: true });
      nodes.set(taskId, { id: taskId, kind: "task", name: task ? { en: task.title } : {}, resourceId: run.task_id, status: task?.status, available: Boolean(task) });
      connect(agentId, id, "run", "runtime");
      connect(id, taskId, "executes", "runtime");
    }
  }

  const kinds = new Set(options.kinds ?? graphKinds);
  const eligible = (id: string) => id === rootId || kinds.has(nodes.get(id)!.kind);
  // Configuration comes first so a busy run history cannot displace dependencies.
  const orderedEdges = [...edges.values()].sort((a, b) => {
    const left = `${a.layer}:${a.id}`;
    const right = `${b.layer}:${b.id}`;
    return left < right ? -1 : left > right ? 1 : 0;
  });
  const adjacency = new Map<string, { id: string; edgeId: string }[]>();
  for (const edge of orderedEdges) {
    if (!eligible(edge.source) || !eligible(edge.target)) continue;
    for (const [from, to] of [[edge.source, edge.target], [edge.target, edge.source]]) {
      const adjacent = adjacency.get(from) ?? [];
      adjacent.push({ id: to, edgeId: edge.id });
      adjacency.set(from, adjacent);
    }
  }
  const expanded = new Set([rootId, ...(options.expandedIds ?? [])]);
  const queue = [rootId];
  const reached = new Set(queue);
  const parents = new Map<string, string>();
  for (let i = 0; i < queue.length; i++) {
    const id = queue[i];
    // Runs always retain their Task context; Tasks do not expand other runs unless requested.
    if (!expanded.has(id) && nodes.get(id)?.kind !== "run") continue;
    for (const next of adjacency.get(id) ?? []) {
      if (!reached.has(next.id)) {
        reached.add(next.id);
        parents.set(next.id, next.edgeId);
        queue.push(next.id);
      }
    }
  }
  const requested = options.maxNodes ?? 120;
  const limit = Number.isFinite(requested) ? Math.max(1, Math.min(120, Math.floor(requested))) : 120;
  const visible = new Set(queue.slice(0, limit));
  const visibleEdges = orderedEdges.filter((edge) => visible.has(edge.source) && visible.has(edge.target));
  // Keep the spanning tree before applying the edge budget, so no shown node is stranded.
  const treeEdges = new Set([...visible].flatMap((id) => parents.has(id) ? [parents.get(id)!] : []));
  const boundedEdges = [
    ...visibleEdges.filter((edge) => treeEdges.has(edge.id)),
    ...visibleEdges.filter((edge) => !treeEdges.has(edge.id)),
  ].slice(0, 360);
  return {
    rootId,
    nodes: [...visible].map((id) => nodes.get(id)!),
    edges: boundedEdges,
    omitted: reached.size - visible.size,
    omittedEdges: visibleEdges.length - boundedEdges.length,
  };
}

/** Stable slots, not a continuously running force simulation. Retain existing/dragged positions. */
export function layoutGraph(nodes: readonly GraphNode[], rootId: string, previous: Positions = {}): Positions {
  const positions: Positions = { ...previous, [rootId]: { x: 0, y: 0 } };
  const occupied = new Set(Object.values(positions).map((p) => `${p.x},${p.y}`));
  const slot = (index: number): Point => {
    let ring = 1;
    while (index >= ring * 8) { index -= ring * 8; ring++; }
    const angle = (index / (ring * 8)) * Math.PI * 2;
    return { x: Math.round(Math.cos(angle) * ring * 240), y: Math.round(Math.sin(angle) * ring * 180) };
  };
  let index = 0;
  for (const node of [...nodes].sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0)) {
    if (positions[node.id] && Number.isFinite(positions[node.id].x) && Number.isFinite(positions[node.id].y)) continue;
    let point = slot(index++);
    while (occupied.has(`${point.x},${point.y}`)) point = slot(index++);
    occupied.add(`${point.x},${point.y}`);
    positions[node.id] = point;
  }
  return positions;
}

export function clampZoom(value: number): number {
  return Number.isFinite(value) ? Math.max(0.2, Math.min(3, value)) : 1;
}

/** Offset reciprocal relationships so their labels and arrowheads do not overlap. */
export function relationshipCurve(
  from: Point,
  to: Point,
  sourceRadius: number,
  targetRadius: number,
  curved: boolean,
): { path: string; label: Point } {
  const length = Math.hypot(to.x - from.x, to.y - from.y) || 1;
  const dx = (to.x - from.x) / length;
  const dy = (to.y - from.y) / length;
  const offset = curved ? 42 : 0;
  const control = {
    x: (from.x + to.x) / 2 - dy * offset,
    y: (from.y + to.y) / 2 + dx * offset,
  };
  const start = { x: from.x + dx * sourceRadius, y: from.y + dy * sourceRadius };
  const end = { x: to.x - dx * targetRadius, y: to.y - dy * targetRadius };
  return {
    path: `M${start.x} ${start.y} Q${control.x} ${control.y} ${end.x} ${end.y}`,
    label: {
      x: (start.x + 2 * control.x + end.x) / 4,
      y: (start.y + 2 * control.y + end.y) / 4,
    },
  };
}
