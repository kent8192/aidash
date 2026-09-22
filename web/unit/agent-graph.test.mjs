import { test } from "node:test";
import assert from "node:assert/strict";
import { buildAgentGraph, entityKey, layoutGraph, clampZoom } from "../src/agent-graph/model.ts";

const ref = (id, version = "1.0.0") => ({ id, version });
const entry = (kind, id, config = {}, version = "1.0.0") => ({ kind, id, version, name: { en: id }, config });
const model = entry("model", "model");
const tool = entry("tool", "search");
const skill = entry("skill", "research");
const cluster = entry("cluster", "team", { coordinator: ref("other") });
const other = entry("agent", "other", { model: ref("model") });
const agent = entry("agent", "agent", {
  model: ref("model"), tools: [ref("search")], skills: [ref("research")], cluster: ref("team"),
  instructions: "Mentioning other does not create an edge", knowledge_digest: "PRIVATE_DIGEST",
  documents: [{ name: "PRIVATE_FILE.pdf", text: "PRIVATE_TEXT" }],
});
const root = { id: agent.id, version: agent.version };
const base = { nodeId: "aidash://local", root, entries: [agent, model, tool, skill, cluster, other], runs: [], tasks: [] };
const key = (e) => entityKey(base.nodeId, e.kind, e);
const graph = (patch = {}, options = {}) => buildAgentGraph({ ...base, ...patch }, options);
const byId = (g, id) => g.nodes.find((n) => n.entity?.id === id);
const run = (id = "run-1", patch = {}) => ({ id, agent_id: "agent", agent_version: "1.0.0", task_id: "task-1", phase: "RUNNING", control: "ACTIVE", ...patch });

test("uses exact versioned references and typed configuration links", () => {
  const g = graph({ entries: [...base.entries, entry("model", "model", {}, "2.0.0")] });
  assert.equal(g.rootId, key(agent));
  assert.equal(g.nodes.length, 5);
  assert.deepEqual(g.edges.map((e) => e.relation).sort(), ["cluster", "model", "skill", "tool"]);
  assert.ok(g.edges.every((e) => e.layer === "configuration"));
  assert.equal(byId(g, "model").entity.version, "1.0.0");
  assert.equal(byId(g, "other"), undefined);
});

test("never falls back to a newer version or a wrong entity kind", () => {
  const g = graph({ entries: [agent, entry("tool", "model"), entry("model", "model", {}, "2.0.0")] });
  const missing = byId(g, "model");
  assert.equal(missing.available, false);
  assert.equal(missing.entity.version, "1.0.0");
  assert.deepEqual(missing.name, {});
});

test("returns no stale graph when the root is outside the authorized snapshot", () => {
  assert.equal(graph({ entries: [model, tool] }), null);
  assert.equal(graph({ entries: [entry("agent", "agent", {}, "2.0.0")] }), null);
});

test("does not copy documents, instructions, credentials or arbitrary metadata", () => {
  const serialized = JSON.stringify(graph());
  for (const word of ["PRIVATE_DIGEST", "PRIVATE_FILE", "PRIVATE_TEXT", "Mentioning", "instructions", "knowledge_digest"]) {
    assert.ok(!serialized.includes(word), word);
  }
});

test("ignores malformed reference fields rather than guessing IDs from text", () => {
  const malformed = entry("agent", "agent", { model: "model@1.0.0", tools: [null, 1, {}, { id: "search" }], skills: "research", cluster: [] });
  const g = graph({ entries: [malformed, model, tool, skill] });
  assert.equal(g.nodes.length, 1);
  assert.equal(g.edges.length, 0);
  assert.equal(graph({ entries: [entry("agent", "agent", null)] }).nodes.length, 1);
});

test("deduplicates versioned references without collapsing distinct relationship types", () => {
  const duplicate = entry("agent", "agent", { tools: [ref("search"), ref("search")] });
  const g = graph({ entries: [duplicate, tool] });
  assert.equal(g.nodes.length, 2);
  assert.equal(g.edges.length, 1);
});

test("expands explicit incoming and outgoing neighbors without text inference", () => {
  const g = graph({}, { expandedIds: [key(model)] });
  assert.ok(byId(g, "other"));
  assert.ok(g.edges.some((e) => e.source === key(other) && e.target === key(model)));
  assert.ok(!g.edges.some((e) => e.source === key(agent) && e.target === key(other)));
});

test("handles cyclic cluster/coordinator configuration", () => {
  const coordinator = entry("agent", "other", { cluster: ref("team") });
  const g = graph({ entries: [agent, cluster, coordinator] }, { expandedIds: [key(cluster), key(other)] });
  assert.equal(new Set(g.nodes.map((n) => n.id)).size, g.nodes.length);
  assert.ok(g.edges.some((e) => e.relation === "coordinator"));
  assert.ok(g.nodes.length < 10);
});

test("ignores expansions that are not reachable from the root", () => {
  const isolated = entry("agent", "isolated", { tools: [ref("secret")] });
  const g = graph({ entries: [...base.entries, isolated, entry("tool", "secret")] }, { expandedIds: [key(isolated)] });
  assert.equal(byId(g, "isolated"), undefined);
  assert.equal(byId(g, "secret"), undefined);
});

test("joins activity through exact-version runs and their explicit task IDs", () => {
  const g = graph({
    runs: [run(), run("wrong-version", { agent_version: "2.0.0" }), run("unrelated", { agent_id: "other" })],
    tasks: [{ id: "task-1", title: "Investigation", status: "RUNNING" }],
  });
  assert.equal(g.nodes.filter((n) => n.kind === "run").length, 1);
  assert.equal(g.nodes.filter((n) => n.kind === "task").length, 1);
  assert.deepEqual(g.edges.filter((e) => e.layer === "runtime").map((e) => e.relation).sort(), ["executes", "run"]);
  assert.ok(!JSON.stringify(g).includes("wrong-version"));
});

test("distinguishes multiple runs of one task and deduplicates repeated snapshots", () => {
  const g = graph({ runs: [run(), run(), run("run-2")] });
  assert.equal(g.nodes.filter((n) => n.kind === "run").length, 2);
  assert.equal(g.nodes.filter((n) => n.kind === "task").length, 1);
  assert.equal(g.edges.filter((e) => e.relation === "executes").length, 2);
  assert.equal(g.nodes.find((n) => n.kind === "task").available, false);
});

test("runtime toggle and type filters preserve the anchor and remove dangling paths", () => {
  const input = { runs: [run()], tasks: [{ id: "task-1", title: "Task", status: "RUNNING" }] };
  assert.ok(graph(input, { runtime: false }).nodes.every((n) => n.kind !== "run" && n.kind !== "task"));
  const g = graph(input, { kinds: ["model", "task"] });
  assert.deepEqual(g.nodes.map((n) => n.kind).sort(), ["agent", "model"]);
  assert.ok(g.edges.every((e) => g.nodes.some((n) => n.id === e.source) && g.nodes.some((n) => n.id === e.target)));
});

test("filters are applied before the graph budget and truncation is explicit", () => {
  const tools = Array.from({ length: 200 }, (_, i) => entry("tool", `tool-${i}`));
  const many = entry("agent", "agent", { model: ref("model"), tools: tools.map((e) => ref(e.id)) });
  const input = { entries: [many, model, ...tools] };
  const g = graph(input, { maxNodes: 10 });
  assert.equal(g.nodes.length, 10);
  assert.equal(g.omitted, 192);
  const filtered = graph(input, { maxNodes: 10, kinds: ["model"] });
  assert.equal(filtered.nodes.length, 2);
  assert.equal(filtered.omitted, 0);
});

test("limits are clamped and configuration is not displaced by busy runtime activity", () => {
  const g = graph({ runs: Array.from({ length: 150 }, (_, i) => run(`run-${i}`)) });
  assert.equal(g.nodes.length, 120);
  assert.ok(byId(g, "model"));
  assert.ok(byId(g, "search"));
  assert.equal(graph({}, { maxNodes: 0 }).nodes.length, 1);
  assert.ok(graph({}, { maxNodes: NaN }).nodes.length > 1);
});

test("identity includes origin, kind and version without delimiter collisions", () => {
  assert.notEqual(key(agent), entityKey("aidash://remote", "agent", agent));
  assert.notEqual(key(agent), entityKey(base.nodeId, "tool", agent));
  assert.notEqual(entityKey(base.nodeId, "agent", ref("a@b", "c")), entityKey(base.nodeId, "agent", ref("a", "b@c")));
});

test("layout preserves positions through additions and filters, including dragged nodes", () => {
  const g = graph();
  const first = layoutGraph(g.nodes, g.rootId);
  first[key(model)] = { x: 777, y: -20 };
  const expanded = graph({}, { expandedIds: [key(model)] });
  const next = layoutGraph(expanded.nodes, expanded.rootId, first);
  for (const n of g.nodes) assert.deepEqual(next[n.id], first[n.id]);
  assert.deepEqual(next[g.rootId], { x: 0, y: 0 });
  assert.ok(Object.values(next).every((p) => Number.isFinite(p.x) && Number.isFinite(p.y)));
  assert.deepEqual(layoutGraph(g.nodes, g.rootId), layoutGraph([...g.nodes].reverse(), g.rootId));
});

test("viewport zoom never becomes non-finite or unusable", () => {
  assert.equal(clampZoom(100), 3);
  assert.equal(clampZoom(0), 0.2);
  assert.equal(clampZoom(NaN), 1);
  assert.equal(clampZoom(Infinity), 1);
  assert.equal(clampZoom(0.75), 0.75);
});

test("caps dense edges while retaining a connected spanning path for every displayed node", () => {
  const tools = Array.from({ length: 30 }, (_, i) => entry("tool", `dense-${i}`));
  const agents = Array.from({ length: 30 }, (_, i) => entry("agent", i === 0 ? "agent" : `peer-${i}`, { tools: tools.map((t) => ref(t.id)) }));
  const g = graph({ entries: [...tools, ...agents] }, { expandedIds: tools.map(key) });
  assert.equal(g.nodes.length, 60);
  assert.ok(g.edges.length <= 360);
  assert.equal(g.omittedEdges, 900 - g.edges.length);
  const seen = new Set([g.rootId]);
  for (let i = 0; i < g.nodes.length; i++) {
    for (const e of g.edges) {
      if (seen.has(e.source)) seen.add(e.target);
      if (seen.has(e.target)) seen.add(e.source);
    }
  }
  assert.equal(seen.size, g.nodes.length);
});

test("opposite relationship arrows have distinct paths and labels", async () => {
  const { relationshipCurve } = await import("../src/agent-graph/model.ts");
  assert.equal(typeof relationshipCurve, "function");
  const from = { x: 0, y: 0 };
  const to = { x: 240, y: 0 };
  const a = relationshipCurve(from, to, 26, 18, true);
  const b = relationshipCurve(to, from, 18, 26, true);
  assert.notEqual(a.path, b.path);
  assert.ok(a.label.y > 0);
  assert.ok(b.label.y < 0);
  assert.equal(relationshipCurve(from, to, 26, 18, false).label.y, 0);
  assert.ok(!relationshipCurve(from, from, 26, 26, false).path.includes("NaN"));
});
