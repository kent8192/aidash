import { test } from "node:test";
import assert from "node:assert/strict";
import {
  resolveLocation,
  destination,
  parseQuery,
  stringifyQuery,
  chooseChannel,
  relatedChannels,
  taskProgress,
  senderLabel,
  primaryDestinations,
} from "../src/collaboration/model.ts";
const workspaces = [
  { id: "one", title: "First" },
  { id: "two", title: "Second" },
];
const data = {
  nodeId: "aidash://home",
  workspaces,
  tasks: [
    { id: "t1", workspace_id: "one", status: "RUNNING" },
    { id: "t2", workspace_id: "two", status: "COMPLETED" },
  ],
  runs: [
    {
      id: "r1",
      workspace_id: "one",
      task_id: "t1",
      agent_id: "a",
      agent_version: "1",
    },
    {
      id: "r2",
      workspace_id: "two",
      task_id: "t2",
      agent_id: "a",
      agent_version: "1",
    },
  ],
  registry: [
    {
      id: "a",
      version: "1",
      kind: "agent",
      config: {
        model: { id: "m", version: "1" },
        tools: [{ id: "tool", version: "1" }],
      },
    },
  ],
};
test("default navigation has exactly collaboration and graph", () => {
  assert.deepEqual(primaryDestinations, ["collaboration", "graph"]);
  assert.equal(resolveLocation("/").section, "collaboration");
});
test("management deep links resolve under settings", () => {
  const r = resolveLocation("/agents");
  assert.equal(r.section, "settings");
  assert.equal(r.settings, "agents");
  assert.equal(r.legacy, true);
});
test("legacy conversation and workspaces routes lead to collaboration", () => {
  for (const route of [
    "/overview",
    "/tasks",
    "/workspaces",
    "/conversations",
    "/events",
  ]) {
    assert.equal(resolveLocation(route).section, "collaboration");
  }
});
test("legacy mesh opens the primary graph", () =>
  assert.equal(resolveLocation("/mesh").section, "graph"));
test("URL navigation preserves channel and exact entity identity", () => {
  const focus = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    "a/a",
    "1",
  ]);
  const url = destination("graph", { channel: "one", focus });
  const [p, q] = url.split("?");
  assert.equal(resolveLocation(p, `?${q}`).focus, focus);
  assert.equal(resolveLocation(p, `?${q}`).channel, "one");
});
test("router query round trip keeps JSON shaped agent keys as plain strings", () => {
  const focus = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    'review/"[special]:/agent',
    "1.0.0",
  ]);
  const search = stringifyQuery({ channel: "one", focus });
  assert.equal(new URLSearchParams(search).get("focus"), focus);
  assert.deepEqual(parseQuery(search), { channel: "one", focus });
  assert.equal(resolveLocation("/graph", search).focus, focus);
});
test("unrecognized settings section cannot become an arbitrary route", () =>
  assert.equal(
    resolveLocation("/settings", "?view=not-real").settings,
    "node",
  ));
test("explicit inaccessible channel does not silently fall back", () =>
  assert.equal(chooseChannel(workspaces, "private"), undefined));
test("empty channel selection can choose first available channel", () =>
  assert.equal(chooseChannel(workspaces, "")?.id, "one"));
test("task node resolves its authorized channel", () =>
  assert.deepEqual(
    relatedChannels({ kind: "task", available: true, resourceId: "t2" }, data),
    [workspaces[1]],
  ));
test("run node resolves its authorized channel", () =>
  assert.deepEqual(
    relatedChannels({ kind: "run", available: true, resourceId: "r1" }, data),
    [workspaces[0]],
  ));
test("reused exact-version agent offers all related channels, current first", () =>
  assert.deepEqual(
    relatedChannels(
      { kind: "agent", available: true, entity: { id: "a", version: "1" } },
      data,
      "two",
    ),
    [workspaces[1], workspaces[0]],
  ));
test("a different agent version is not a relationship match", () =>
  assert.deepEqual(
    relatedChannels(
      { kind: "agent", available: true, entity: { id: "a", version: "2" } },
      data,
    ),
    [],
  ));
test("configured tool follows explicit versioned agent references", () =>
  assert.deepEqual(
    relatedChannels(
      { kind: "tool", available: true, entity: { id: "tool", version: "1" } },
      data,
    ),
    workspaces,
  ));
test("missing nodes never expose channel associations", () =>
  assert.deepEqual(
    relatedChannels(
      { kind: "agent", available: false, entity: { id: "a", version: "1" } },
      data,
    ),
    [],
  ));
test("channel association never returns inaccessible workspaces", () =>
  assert.deepEqual(
    relatedChannels(
      { kind: "agent", available: true, entity: { id: "a", version: "1" } },
      { ...data, workspaces: [workspaces[1]] },
    ),
    [workspaces[1]],
  ));
test("task progress counts work but does not declare goal completion", () =>
  assert.deepEqual(taskProgress(data.tasks, "one"), {
    total: 1,
    completed: 0,
    active: 1,
    attention: 0,
  }));
test("versioned agent IDs and scoped humans get distinct sender labels", () => {
  assert.deepEqual(senderLabel("aidash://home/agents/a@1"), {
    kind: "agent",
    name: "a@1",
  });
  assert.deepEqual(senderLabel("alice"), { kind: "human", name: "alice" });
});
test("remote workspace UUID collisions do not link a local channel", () => {
  const foreign = {
    ...data,
    runs: data.runs.map((run) => ({ ...run, home_node: "aidash://other" })),
  };
  assert.deepEqual(
    relatedChannels(
      { kind: "agent", available: true, entity: { id: "a", version: "1" } },
      foreign,
    ),
    [],
  );
  assert.deepEqual(
    relatedChannels(
      { kind: "run", available: true, resourceId: "r1" },
      foreign,
    ),
    [],
  );
});
test("cluster coordinator contributes related channels even without a member reference", () => {
  const registry = [
    ...data.registry,
    {
      id: "cluster",
      version: "1",
      kind: "cluster",
      config: { coordinator: { id: "a", version: "1" } },
    },
  ];
  assert.deepEqual(
    relatedChannels(
      {
        kind: "cluster",
        available: true,
        entity: { id: "cluster", version: "1" },
      },
      { ...data, registry },
    ),
    workspaces,
  );
});
test("a human display identifier containing an agents path is not an agent identity", () => {
  assert.deepEqual(senderLabel("alice/agents/not-an-agent"), {
    kind: "human",
    name: "alice/agents/not-an-agent",
  });
});
