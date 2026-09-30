import assert from "node:assert/strict";
import test from "node:test";
import {
  graphRegionPositions,
  graphRegions,
  regionForNode,
  workspaceGraph,
} from "../src/collaboration/graph-regions.ts";
import { filterMeshGraph, meshKinds } from "../src/collaboration/mesh-model.ts";

const home = "aidash://a";
const b = "aidash://b";
const c = "aidash://c";
const key = (node, kind, id) => JSON.stringify(["resource", node, kind, id]);
const entity = (node, id, version = "1.0.0") =>
  JSON.stringify(["entity", node, "agent", id, version]);
const shared = (node, kind, id, workspaceId) => ({
  id: key(node, kind, id),
  kind,
  nodeId: node,
  resourceId: id,
  workspaceId,
  name: { en: id },
  available: true,
  projected: node !== home,
  remote: node !== home,
});
const agent = (node, id, version = "1.0.0") => ({
  id: entity(node, id, version),
  kind: "agent",
  nodeId: node,
  entity: { id, version },
  name: { en: "Same display name" },
  available: true,
  projected: node !== home,
  remote: node !== home,
});
const edge = (source, target, relation) => ({
  id: JSON.stringify([source, relation, target]),
  source,
  target,
  relation,
  layer: "activity",
});
const graph = () => {
  const nodes = [
    shared(home, "workspace", "work", "work"),
    shared(home, "task", "task", "work"),
    shared(home, "conversation", "talk", "work"),
    shared(b, "workspace", "work", "work"),
    shared(b, "task", "unrelated", "work"),
    agent(home, "same"),
    agent(b, "same"),
    agent(c, "same", "2.0.0"),
    agent(c, "idle"),
    shared(home, "run", "local-run", "work"),
    shared(b, "run", "same-run", "work"),
    shared(c, "run", "same-run", "work"),
    {
      ...shared(home, "tool", "tool", undefined),
      kind: "tool",
    },
    shared(b, "remote", b, undefined),
    shared(c, "remote", c, undefined),
  ];
  const edges = [
    edge(key(home, "task", "task"), entity(home, "same"), "assigned"),
    edge(entity(home, "same"), key(home, "run", "local-run"), "executes"),
    edge(entity(b, "same"), key(b, "run", "same-run"), "executes"),
    edge(key(b, "run", "same-run"), key(home, "task", "task"), "executes"),
    edge(entity(c, "same", "2.0.0"), key(c, "run", "same-run"), "executes"),
    edge(key(c, "run", "same-run"), key(home, "task", "task"), "executes"),
    edge(entity(home, "same"), key(home, "tool", "tool"), "tool"),
  ];
  return { nodes, edges, omitted: 0, omittedEdges: 0 };
};
const state = {
  tasks: [
    {
      id: "task",
      workspace_id: "work",
      owner: `${home}/agents/same@1.0.0`,
      created_by: "person",
    },
  ],
  conversations: [],
};

test("selected Home Workspace keeps only proven Agents and puts peer Runs in their executing regions", () => {
  const scoped = workspaceGraph(graph(), home, "work", state);
  assert.ok(scoped.nodes.some((node) => node.id === key(home, "task", "task")));
  assert.ok(
    !scoped.nodes.some((node) => node.id === key(b, "task", "unrelated")),
  );
  assert.ok(!scoped.nodes.some((node) => node.id === entity(c, "idle")));
  assert.ok(!scoped.nodes.some((node) => node.kind === "remote"));
  assert.equal(scoped.nodes.filter((node) => node.kind === "agent").length, 3);
  assert.equal(scoped.nodes.filter((node) => node.kind === "run").length, 3);
  assert.equal(
    regionForNode(
      scoped.nodes.find((node) => node.id === key(b, "run", "same-run")),
    ).nodeId,
    b,
  );
  const regions = graphRegions(scoped, home);
  assert.deepEqual(
    regions
      .filter((region) => region.kind === "execution")
      .map((region) => region.nodeId),
    [home, b, c],
  );
  assert.deepEqual(
    regions
      .filter((region) => region.kind === "shared")
      .map((region) => region.nodeId),
    [home],
  );
});

test("mode and kind filters do not erase the evidence that made an Agent Workspace-related", () => {
  const scoped = workspaceGraph(graph(), home, "work", state);
  for (const mode of ["mesh", "collaboration", "knowledge", "topology"]) {
    const filtered = filterMeshGraph(scoped, {
      mode,
      kinds: meshKinds.filter((kind) => kind !== "run" && kind !== "remote"),
      relations: ["contains"],
      query: "",
    });
    assert.ok(
      filtered.nodes.some((node) => node.id === entity(b, "same")),
      mode,
    );
    assert.ok(
      filtered.nodes.some((node) => node.id === entity(c, "same", "2.0.0")),
      mode,
    );
  }
});

test("All Workspaces keeps Home-qualified ownership and Node-qualified execution separate", () => {
  const scoped = workspaceGraph(graph(), home, "", state);
  const regions = graphRegions(scoped, home);
  assert.deepEqual(
    regions
      .filter((region) => region.kind === "shared")
      .map((region) => [region.nodeId, region.workspaceId]),
    [
      [home, "work"],
      [b, "work"],
    ],
  );
  assert.equal(
    regions.filter(
      (region) => region.kind === "execution" && region.nodeId === b,
    ).length,
    1,
  );
  assert.notEqual(key(b, "run", "same-run"), key(c, "run", "same-run"));
  const positions = graphRegionPositions(scoped, home);
  for (const node of scoped.nodes)
    assert.ok(
      Number.isFinite(positions[node.id]?.x) &&
        Number.isFinite(positions[node.id]?.y),
      node.id,
    );
  assert.ok(positions[entity(home, "same")].x < positions[entity(b, "same")].x);
  assert.ok(
    positions[key(home, "workspace", "work")].y >
      positions[key(home, "run", "local-run")].y,
  );
});
