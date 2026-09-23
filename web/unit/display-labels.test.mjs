import { test } from "node:test";
import assert from "node:assert/strict";
import { disambiguateLabels } from "../src/display-labels.ts";
import { presentRecord } from "../src/record-presentation.ts";

test("equal names get distinct, order-independent labels without exposing IDs", () => {
  const entries = [
    { id: "entry-b", name: "Research model · 1.0.0" },
    { id: "entry-a", name: "Research model · 1.0.0" },
    { id: "entry-c", name: "Other model · 1.0.0" },
  ];
  const labels = (items) =>
    disambiguateLabels(
      items,
      (item) => item.id,
      (item) => item.name,
    );
  const sorted = (items) =>
    [...labels(items)].sort(([a], [b]) => a.localeCompare(b));
  assert.deepEqual(sorted(entries), sorted([...entries].reverse()));
  assert.equal(labels(entries).get("entry-a"), "Research model · 1.0.0 (#1)");
  assert.equal(labels(entries).get("entry-b"), "Research model · 1.0.0 (#2)");
  assert.equal(labels(entries).get("entry-c"), "Other model · 1.0.0");
  assert.ok(
    [...labels(entries).values()].every((label) => !label.includes("entry-")),
  );
});

test("duplicate labels are scoped per node", () => {
  const agents = [
    { node: "peer", id: "b", name: "Researcher" },
    { node: "peer", id: "a", name: "Researcher" },
    { node: "home", id: "c", name: "Researcher" },
  ];
  const labels = disambiguateLabels(
    agents,
    (agent) => `${agent.node}/${agent.id}`,
    (agent) => agent.name,
    (agent) => agent.node,
  );
  assert.equal(labels.get("peer/a"), "Researcher (#1)");
  assert.equal(labels.get("peer/b"), "Researcher (#2)");
  assert.equal(labels.get("home/c"), "Researcher");
});

test("record presentation preserves user-authored config and schema examples", () => {
  const id = "019a0000-0000-7000-8000-000000000001";
  const sample = { id, version: "1.0.0" };
  const record = {
    id,
    model: sample,
    config: { example: sample },
    schema: { example: sample },
  };
  const presented = presentRecord(
    record,
    new Map([[`${id}@1.0.0`, "Research model · 1.0.0"]]),
    "Unavailable",
  );
  assert.deepEqual(presented, {
    model: "Research model · 1.0.0",
    config: { example: sample },
    schema: { example: sample },
  });
  assert.deepEqual(record.config.example, sample);
});

test("typed registry references are named while unrelated configuration stays intact", () => {
  const model = { id: "model-id", version: "1.0.0" };
  const tool = { id: "tool-id", version: "2.0.0" };
  const example = { id: "business-record", version: "customer-version" };
  const labels = new Map([
    ["model-id@1.0.0", "Research model · 1.0.0"],
    ["tool-id@2.0.0", "Search tool · 2.0.0"],
  ]);
  const value = {
    kind: "agent",
    config: {
      model,
      tools: [tool],
      instructions: "Keep the example",
      knowledge_digest: "opaque",
      example,
    },
    schema: { example },
  };
  assert.deepEqual(presentRecord(value, labels, "Unavailable"), {
    kind: "agent",
    config: {
      model: "Research model · 1.0.0",
      tools: ["Search tool · 2.0.0"],
      instructions: "Keep the example",
      example,
    },
    schema: { example },
  });
  assert.deepEqual(
    presentRecord(
      { kind: "model", config: { model, example } },
      labels,
      "Unavailable",
    ),
    {
      kind: "model",
      config: { model, example },
    },
  );
});
