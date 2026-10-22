import { test } from "node:test";
import assert from "node:assert/strict";
import { groupByKind } from "../src/registry-kinds.ts";

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
