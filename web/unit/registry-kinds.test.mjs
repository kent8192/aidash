import { test } from "node:test";
import assert from "node:assert/strict";
import { focusedVersion, groupByKind } from "../src/registry-kinds.ts";

test("Registry entries group by kind in reading order", () => {
  const entries = [
    { id: "search", kind: "tool" },
    { id: "future", kind: "workflow" },
    { id: "writer", kind: "agent" },
    { id: "gpt", kind: "model" },
    { id: "reader", kind: "agent" },
  ];
  assert.deepEqual(
    groupByKind(entries, (entry) => entry.kind).map(([kind, items]) => [
      kind,
      items.map((item) => item.id),
    ]),
    [
      ["agent", ["writer", "reader"]],
      ["model", ["gpt"]],
      ["tool", ["search"]],
      ["workflow", ["future"]],
    ],
  );
});

test("a settings link opens only the Registry version it names", () => {
  const versions = ["1.0.0", "2.0.0"].map((version) => ({
    entry: { id: "tenant.model", version },
  }));
  assert.equal(focusedVersion(versions, "tenant.model@2.0.0"), versions[1]);
  assert.equal(focusedVersion(versions, "semantic"), null);
  assert.equal(focusedVersion(versions, ""), null);
});
