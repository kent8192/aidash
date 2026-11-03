import { test } from "node:test";
import assert from "node:assert/strict";
import {
  chooseNode,
  nodeLabel,
  nodeStorageKey,
  nodeView,
  parseNodeAddress,
  isLocalRun,
} from "../src/node-selection.ts";

const node = (name, options = {}) => ({
  name,
  label: options.label ?? name,
  address: `aidash://home/${name}`,
  default: options.default ?? false,
  state: "live",
});
const nodes = [node("first", { default: true }), node("second")];

test("the remembered Node is used while the Tenant still holds it", () => {
  assert.equal(chooseNode(nodes, "second")?.name, "second");
});

test("a forgotten or unknown Node falls back to the default Node", () => {
  assert.equal(chooseNode(nodes, null)?.name, "first");
  assert.equal(chooseNode(nodes, "gone")?.name, "first");
});

test("a Tenant with no readable Nodes has no selection", () => {
  assert.equal(chooseNode([], "first"), undefined);
});

test("each Tenant remembers its own Node", () => {
  assert.notEqual(nodeStorageKey("one"), nodeStorageKey("two"));
});

const state = () => ({
  access: { kind: "subject", tenant: "acme", subject: "ryota" },
  deployment: {
    id: "aidash://home",
    endpoint: "http://home",
    protocol_version: "0.4",
    key: null,
  },
  registry: [{ id: "agent" }],
  workspaces: [
    { id: "w1", node: "first" },
    { id: "w2", node: "second" },
  ],
  tasks: [
    { id: "t1", workspace_id: "w1" },
    { id: "t2", workspace_id: "w2" },
  ],
  runs: [
    { id: "r1", node: "first", workspace_id: "w1" },
    { id: "r2", node: "second", workspace_id: "w2" },
    { id: "r3", node: "first", workspace_id: "w2" },
    { id: "r4", node: "second", workspace_id: "remote-workspace" },
  ],
  human_requests: [
    { id: "h1", workspace_id: "w1" },
    { id: "h2", workspace_id: "w2" },
  ],
  conversations: [
    { id: "c1", workspace_id: "w1" },
    { id: "c2", workspace_id: "w2" },
  ],
  events: [
    { id: "e1", node: "first" },
    { id: "e2", node: "second" },
    { id: "e3", node: null },
  ],
  artifacts: [
    { id: "a1", workspace_id: "w1" },
    { id: "a2", workspace_id: "w2" },
  ],
});

test("a Node's view holds only what its Workspaces own, and names the Node", () => {
  const view = nodeView(state(), nodes[1]);
  assert.equal(view.node.id, "aidash://home/second");
  assert.equal(view.node.endpoint, "http://home");
  assert.deepEqual(
    view.workspaces.map((value) => value.id),
    ["w2"],
  );
  const expected = {
    tasks: ["t2"],
    runs: ["r2", "r3"],
    human_requests: ["h2"],
    conversations: ["c2"],
    artifacts: ["a2"],
  };
  for (const [key, ids] of Object.entries(expected))
    assert.deepEqual(
      view[key].map((value) => value.id),
      ids,
      key,
    );
  assert.deepEqual(
    view.events.map((value) => value.id),
    ["e2", "e3"],
    "a Tenant-level event has no Node and belongs to every view",
  );
  assert.deepEqual(view.registry, [{ id: "agent" }]);
});

test("a Deployment Administrator has no Node, so State stays whole and names the Deployment", () => {
  const whole = state();
  const view = nodeView(whole, undefined);
  assert.deepEqual(view.node, { id: "aidash://home", endpoint: "http://home" });
  assert.deepEqual(view.workspaces, whole.workspaces);
  assert.deepEqual(view.runs, whole.runs);
});

test("a Node is shown by its label, or by its name until the list arrives", () => {
  const labelled = [node("first", { label: "Lab" })];
  assert.equal(nodeLabel(labelled, "first"), "Lab");
  assert.equal(nodeLabel(labelled, "other"), "other");
});

test("a Node address splits into its Deployment and Node name", () => {
  assert.deepEqual(parseNodeAddress("aidash://home/brave-otter-ab12cd34"), {
    deployment: "home",
    node: "brave-otter-ab12cd34",
  });
  assert.equal(parseNodeAddress("aidash://home"), null);
  assert.equal(parseNodeAddress("aidash://home/a/b"), null);
  assert.equal(parseNodeAddress("https://home/a"), null);
});

test("subjects monitor only Runs homed on their selected Node", () => {
  const subject = {
    access: { kind: "subject" },
    node: { id: "aidash://home/first" },
  };
  assert.equal(isLocalRun({ home_node: "aidash://home/first" }, subject), true);
  assert.equal(
    isLocalRun({ home_node: "aidash://home/second" }, subject),
    false,
  );
  assert.equal(
    isLocalRun({ home_node: "aidash://other/first" }, subject),
    false,
  );
});

test("Deployment Administrators monitor every Node of their own Deployment", () => {
  const administrator = {
    access: { kind: "deployment_administrator" },
    node: { id: "aidash://home" },
  };
  assert.equal(
    isLocalRun({ home_node: "aidash://home/first" }, administrator),
    true,
  );
  assert.equal(
    isLocalRun({ home_node: "aidash://home/second" }, administrator),
    true,
  );
});

test("Deployment Administrators do not treat a foreign Deployment's Run as local", () => {
  const administrator = {
    access: { kind: "deployment_administrator" },
    node: { id: "aidash://home" },
  };
  assert.equal(
    isLocalRun({ home_node: "aidash://other/first" }, administrator),
    false,
  );
  assert.equal(
    isLocalRun({ home_node: "aidash://home-other/first" }, administrator),
    false,
  );
});

test("Deployment Administrator monitoring requires a valid Node address", () => {
  const administrator = {
    access: { kind: "deployment_administrator" },
    node: { id: "aidash://home" },
  };
  for (const home_node of [
    "aidash://home",
    "aidash://home/first/extra",
    "https://home/first",
  ])
    assert.equal(isLocalRun({ home_node }, administrator), false);
});

test("a remembered deleted Node and deleted default never enter live selection", () => {
  const lifecycle = [
    { name: "deleted", default: true, state: "deleted" },
    { name: "deleting", default: false, state: "deleting" },
    { name: "purged", default: false, state: "purged" },
    { name: "live", default: false, state: "live" },
  ];
  assert.equal(chooseNode(lifecycle, "deleted")?.name, "live");
  assert.equal(chooseNode(lifecycle, "purged")?.name, "live");
  assert.equal(chooseNode(lifecycle.slice(0, 3), "deleted"), undefined);
});
