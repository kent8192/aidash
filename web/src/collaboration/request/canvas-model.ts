import type { EntityRef, HumanRequest, Task } from "../../types";
import { terminalRun, type LocatedRun } from "../workspace-model";

/** One agent taking part in this request on one execution node. */
export type Participant = {
  key: string;
  node: string;
  agent: EntityRef;
  /** Most relevant run: the latest live run, otherwise the latest run. */
  run?: LocatedRun;
  /** Task shown on the tile: the run's task, otherwise the owner's open task. */
  task?: Task;
  depth: number;
};

export type CanvasEdge = {
  key: string;
  from: string;
  to: string;
  /** Downstream task the hand-off feeds. */
  task: Task;
  /** Dependency the downstream task is waiting for, when still unfinished. */
  waitingFor?: Task;
  tone: "idle" | "live" | "waiting";
};

export type NodeZone = {
  node: string;
  local: boolean;
  /** Columns ordered by dependency depth; each column keeps snapshot order. */
  columns: Participant[][];
};

export type PendingRequest = { request: HumanRequest; node: string };

export const runDisplayStatus = (run: LocatedRun["run"]) =>
  run.control === "PAUSED" ? "PAUSED" : run.phase;

const participantKey = (node: string, agent: EntityRef) =>
  JSON.stringify([node, agent.id, agent.version]);

/** Task owners are `aidash://node/agents/id@version` entity keys. */
export function parseOwner(
  owner: string | null,
): { node: string; agent: EntityRef } | undefined {
  if (!owner) return undefined;
  const marker = owner.indexOf("/agents/");
  if (!owner.startsWith("aidash://") || marker <= "aidash://".length)
    return undefined;
  const reference = owner.slice(marker + "/agents/".length);
  const separator = reference.lastIndexOf("@");
  if (separator <= 0) return undefined;
  return {
    node: owner.slice(0, marker),
    agent: {
      id: reference.slice(0, separator),
      version: reference.slice(separator + 1),
    },
  };
}

function taskDepths(tasks: readonly Task[]) {
  const byId = new Map(tasks.map((task) => [task.id, task]));
  const depths = new Map<string, number>();
  const visit = (task: Task, trail: Set<string>): number => {
    const known = depths.get(task.id);
    if (known !== undefined) return known;
    if (trail.has(task.id)) return 0;
    trail.add(task.id);
    const depth = Math.max(
      0,
      ...(task.dependencies ?? []).flatMap((id) => {
        const dependency = byId.get(id);
        return dependency ? [visit(dependency, trail) + 1] : [];
      }),
    );
    trail.delete(task.id);
    depths.set(task.id, depth);
    return depth;
  };
  for (const task of tasks) visit(task, new Set());
  return depths;
}

/** Unfinished tasks are shown before finished ones when an agent has no run. */
const unfinished = ["RUNNING", "CLAIMED", "BLOCKED", "OPEN", "FAILED"];

/**
 * Projects the request's snapshot (tasks, runs and owners) onto a deterministic canvas:
 * one participant per agent and execution node, grouped by node, columns by dependency depth.
 */
export function requestCanvas(
  tasks: readonly Task[],
  runs: readonly LocatedRun[],
  localNode: string,
) {
  const depths = taskDepths(tasks);
  const byId = new Map(tasks.map((task) => [task.id, task]));
  const participants = new Map<string, Participant>();
  const ensure = (node: string, agent: EntityRef) => {
    const key = participantKey(node, agent);
    let participant = participants.get(key);
    if (!participant) {
      participant = { key, node, agent, depth: 0 };
      participants.set(key, participant);
    }
    return participant;
  };
  const ordered = [...runs].sort((a, b) =>
    b.run.updated_at.localeCompare(a.run.updated_at),
  );
  const runOwner = new Map<string, string>();
  for (const item of ordered) {
    const participant = ensure(item.node, {
      id: item.run.agent_id,
      version: item.run.agent_version,
    });
    if (!runOwner.has(item.run.task_id))
      runOwner.set(item.run.task_id, participant.key);
    if (
      !participant.run ||
      (terminalRun(participant.run.run) && !terminalRun(item.run))
    )
      participant.run = item;
  }
  const ownerOf = new Map<string, string>();
  for (const task of tasks) {
    const fromRun = runOwner.get(task.id);
    if (fromRun) {
      ownerOf.set(task.id, fromRun);
      continue;
    }
    const owner = parseOwner(task.owner);
    if (owner) ownerOf.set(task.id, ensure(owner.node, owner.agent).key);
  }
  for (const participant of participants.values()) {
    const runTask = participant.run && byId.get(participant.run.run.task_id);
    const owned = tasks
      .filter((task) => ownerOf.get(task.id) === participant.key)
      .sort(
        (a, b) =>
          Number(!unfinished.includes(a.status)) -
          Number(!unfinished.includes(b.status)),
      );
    participant.task = runTask ?? owned[0];
    participant.depth = participant.task
      ? (depths.get(participant.task.id) ?? 0)
      : 0;
  }
  const edges: CanvasEdge[] = [];
  for (const task of tasks) {
    const to = ownerOf.get(task.id);
    for (const id of task.dependencies ?? []) {
      const dependency = byId.get(id);
      const from = ownerOf.get(id);
      if (!dependency || !from || !to || from === to) continue;
      const waiting =
        dependency.status !== "COMPLETED" &&
        !["COMPLETED", "FAILED", "CANCELLED", "ABANDONED"].includes(
          task.status,
        );
      edges.push({
        key: `${id}:${task.id}`,
        from,
        to,
        task,
        waitingFor: waiting ? dependency : undefined,
        tone: waiting
          ? "waiting"
          : ["RUNNING", "CLAIMED"].includes(task.status)
            ? "live"
            : "idle",
      });
    }
  }
  const zones = new Map<string, Participant[]>();
  for (const participant of participants.values())
    zones.set(participant.node, [
      ...(zones.get(participant.node) ?? []),
      participant,
    ]);
  const nodeZones: NodeZone[] = [...zones.entries()]
    .sort(
      ([a], [b]) =>
        Number(b === localNode) - Number(a === localNode) || a.localeCompare(b),
    )
    .map(([node, members]) => {
      const columns: Participant[][] = [];
      for (const participant of members)
        (columns[participant.depth] ??= []).push(participant);
      return {
        node,
        local: node === localNode,
        columns: columns.filter(Boolean),
      };
    });
  return { participants, edges, zones: nodeZones, ownerOf };
}

/** Pending requests keyed by the participant whose run asked; unmatched ones stay unanchored. */
export function anchorRequests(
  requests: readonly PendingRequest[],
  participants: ReadonlyMap<string, Participant>,
  runs: readonly LocatedRun[],
) {
  const anchored = new Map<string, PendingRequest[]>();
  const loose: PendingRequest[] = [];
  for (const item of requests) {
    const run = runs.find(
      (candidate) =>
        candidate.run.id === item.request.run_id &&
        candidate.node === item.node,
    );
    const key =
      run &&
      participantKey(run.node, {
        id: run.run.agent_id,
        version: run.run.agent_version,
      });
    if (key && participants.has(key))
      anchored.set(key, [...(anchored.get(key) ?? []), item]);
    else loose.push(item);
  }
  return { anchored, loose };
}
