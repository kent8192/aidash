import assert from "node:assert/strict";
import test from "node:test";
import {
  buildMeshGraph,
  eventReferences,
  filterMeshGraph,
  inWindow,
  meshKinds,
  meshPositions,
  nodeEvents,
} from "../src/collaboration/mesh-model.ts";
import { meshScene } from "../tests/mesh-scene.mjs";
const now = Date.parse("2026-09-24T09:00:00Z");
const scene = () => meshScene(now);
const project = (s) =>
  buildMeshGraph(s.data, {
    channel: "product-lab",
    discovery: s.discovery,
    now,
  });
const node = (g, id) =>
  g.nodes.find((n) => n.entity?.id === id || n.resourceId === id);
const visible = (g, options = {}) =>
  filterMeshGraph(g, { mode: "mesh", kinds: meshKinds, query: "", ...options });

test("projects explicit workspace, task hierarchy, dependencies, tools, artifacts and conversation links", () => {
  const graph = project(scene());
  for (const kind of meshKinds)
    assert.ok(
      graph.nodes.some((n) => n.kind === kind),
      kind,
    );
  for (const relation of [
    "goal",
    "depends",
    "produces",
    "participates",
    "coordinates",
    "tool",
    "model",
    "member",
    "assigned",
    "executes",
  ])
    assert.ok(
      graph.edges.some((e) => e.relation === relation),
      relation,
    );
  assert.ok(
    graph.edges.some(
      (e) =>
        e.source === node(graph, "t-101").id &&
        e.target === node(graph, "t-102").id &&
        e.relation === "contains",
    ),
  );
  assert.equal(node(graph, "planner").parent, node(graph, "local-cluster").id);
  assert.equal(node(graph, "researcher").status, "COMPLETED");
  assert.ok(
    graph.edges.every(
      (e) =>
        graph.nodes.some((n) => n.id === e.source) &&
        graph.nodes.some((n) => n.id === e.target),
    ),
  );
});
test("uses exact versions, preserves remote identities, and does not invent health", () => {
  const s = scene();
  s.data.registry[0].config.tools.push({ id: "not-found", version: "1.0.0" });
  s.data.registry.push({
    ...s.data.registry[6],
    id: "not-found",
    version: "2.0.0",
  });
  s.discovery.agents.push({
    node_id: s.data.peers[0].node_id,
    entity: { ...s.data.registry[0] },
  });
  const g = project(s);
  const missing = g.nodes.find(
    (n) => n.entity?.id === "not-found" && n.entity.version === "1.0.0",
  );
  assert.equal(missing.available, false);
  assert.deepEqual(missing.name, {});
  const planners = g.nodes.filter((n) => n.entity?.id === "planner");
  assert.equal(new Set(planners.map((n) => n.id)).size, 2);
  assert.equal(planners.find((n) => n.remote).status, "DISCOVERED");
  assert.ok(
    g.nodes.every((n) => n.status !== "HEALTHY" && n.status !== "ONLINE"),
  );
});
test("subject views cannot join discovery or remote runs, and renderers receive no private payload", () => {
  const s = scene();
  s.data.access = { kind: "subject", tenant: "acme", subject: "ryota" };
  s.data.registry[0].config.knowledge = { credential: "PRIVATE_SECRET" };
  s.data.artifacts[0].content = "PRIVATE_CONTENT";
  s.data.runs[0].context = { messages: "PRIVATE_MESSAGES" };
  const g = project(s);
  assert.ok(!g.nodes.some((n) => n.remote || n.kind === "remote"));
  assert.ok(!JSON.stringify(g).includes("PRIVATE_"));
});
test("an unavailable workspace never falls back to another workspace's activity", () => {
  const s = scene();
  const g = buildMeshGraph(s.data, {
    channel: "revoked",
    discovery: s.discovery,
    now,
  });
  assert.ok(
    !g.nodes.some((n) =>
      [
        "task",
        "workspace",
        "goal",
        "human",
        "artifact",
        "conversation",
      ].includes(n.kind),
    ),
  );
  assert.ok(
    g.nodes
      .filter((n) => n.kind === "agent" && !n.remote)
      .every((n) => !n.status),
  );
});
test("kind, relation, search and focus filters remove dangling links and preserve focus under budget", () => {
  const g = project(scene());
  const filtered = visible(g, {
    kinds: ["agent", "task"],
    query: "Planner",
    relations: ["executes"],
  });
  assert.ok(filtered.nodes.some((n) => n.entity?.id === "planner"));
  assert.ok(filtered.nodes.every((n) => ["agent", "task"].includes(n.kind)));
  assert.ok(filtered.edges.every((e) => e.relation === "executes"));
  const capped = visible(g, {
    focus: node(g, "builder").id,
    maxNodes: 2,
    maxEdges: 0,
  });
  assert.equal(capped.nodes.length, 2);
  assert.equal(capped.nodes[0].id, node(g, "builder").id);
  assert.ok(capped.omitted > 0);
  assert.ok(
    capped.nodes.every(
      (n) => !n.parent || capped.nodes.some((p) => p.id === n.parent),
    ),
  );
  assert.equal(visible(g, { query: "no-such-node" }).nodes.length, 0);
});
test("layouts remain stable on reordered snapshots and produce finite, separated leaf positions", () => {
  const g = project(scene());
  for (const mode of [
    "mesh",
    "collaboration",
    "knowledge",
    "execution",
    "topology",
  ]) {
    const filtered = visible(g, { mode });
    const positions = meshPositions(filtered, mode);
    assert.deepEqual(
      positions,
      meshPositions(
        { ...filtered, nodes: [...filtered.nodes].reverse() },
        mode,
      ),
    );
    const points = filtered.nodes
      .filter((n) => n.kind !== "cluster")
      .map((n) => positions[n.id]);
    assert.ok(
      points.every((p) => Number.isFinite(p.x) && Number.isFinite(p.y)),
    );
    assert.equal(
      new Set(points.map((p) => `${p.x}:${p.y}`)).size,
      points.length,
      mode,
    );
  }
});
test("time windows filter completed activity, retain active execution and match exact event resource IDs", () => {
  const s = scene();
  s.data.runs[1].updated_at = "2020-01-01T00:00:00Z";
  s.data.runs[0].updated_at = "2020-01-01T00:00:00Z";
  const g = buildMeshGraph(s.data, { hours: 24, now });
  assert.equal(node(g, "researcher").status, undefined);
  assert.equal(node(g, "planner").status, "THINKING");
  s.data.events.push({
    ...s.data.events[0],
    id: "wrong-task",
    data: { task_id: "t-1010" },
  });
  assert.ok(
    !nodeEvents(node(g, "t-101"), g, s.data, 1, now).some(
      (e) => e.id === "wrong-task",
    ),
  );
  assert.equal(inWindow("invalid", 1, now), false);
  assert.equal(inWindow(new Date(now + 1000).toISOString(), 1, now), false);
  assert.equal(inWindow("2020-01-01", 0, now), true);
});
test("revoked entities and disabled peers remove cached names and remote status", () => {
  const s = scene();
  s.data.registry = s.data.registry.filter((e) => e.id !== "builder");
  s.data.peers[0].enabled = false;
  const g = project(s);
  assert.equal(node(g, "builder"), undefined);
  assert.ok(!g.nodes.some((n) => n.remote));
  assert.ok(!JSON.stringify(g).includes("Builder Agent"));
});

test("matches server event envelopes for task creation, completion, publication and run creation", () => {
  const s = scene();
  const task = s.data.tasks[0];
  const artifact = s.data.artifacts[0];
  const run = s.data.runs[0];
  const event = s.data.events[0];
  assert.equal(
    eventReferences({ ...event, kind: "task.created", data: task }).task_id,
    task.id,
  );
  assert.deepEqual(
    eventReferences({
      ...event,
      kind: "task.completed",
      data: { task, artifact },
    }),
    { task_id: task.id, artifact_id: artifact.id, run_id: undefined },
  );
  assert.equal(
    eventReferences({ ...event, kind: "artifact.published", data: artifact })
      .artifact_id,
    artifact.id,
  );
  assert.equal(
    eventReferences({ ...event, kind: "run.created", data: run }).run_id,
    run.id,
  );
  assert.equal(
    eventReferences({ ...event, kind: "unrelated", data: { id: task.id } })
      .task_id,
    undefined,
  );
  const graph = project(s);
  s.data.events = [
    { ...event, id: "created", kind: "task.created", data: task },
    { ...event, id: "dependency", kind: "task.created", data: s.data.tasks[1] },
  ];
  assert.deepEqual(
    nodeEvents(node(graph, task.id), graph, s.data, 24, now).map((e) => e.id),
    ["created"],
  );
});

test("foreign task homes do not create local activity links, and task IDs can be searched", () => {
  const s = scene();
  s.data.runs = [{ ...s.data.runs[0], home_node: "aidash://another-home" }];
  const g = project(s);
  assert.ok(!g.edges.some((edge) => edge.relation === "executes"));
  assert.equal(node(g, "planner").status, undefined);
  assert.ok(
    visible(g, { query: "t-102" }).nodes.some((n) => n.resourceId === "t-102"),
  );
  assert.equal(inWindow("not-a-date", 0, now), false);
});
