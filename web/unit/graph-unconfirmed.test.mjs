import assert from "node:assert/strict";
import test from "node:test";
import { graphUnconfirmed } from "../src/collaboration/graph-unconfirmed.ts";

test("graph warning counts the current pages under their answering Nodes", () => {
  const row = (count) => ({
    node: "aidash://b",
    kind: "task",
    workspace_id: "w",
    count,
  });
  const pages = {
    "aidash://b": { scope: "current", page: { unconfirmed: [row(2), row(3)] } },
    "aidash://old": { scope: "previous", page: { unconfirmed: [row(9)] } },
    "aidash://empty": { scope: "current", page: { unconfirmed: [] } },
  };
  assert.deepEqual(graphUnconfirmed(pages, "current"), [
    { node: "aidash://b", count: 5 },
  ]);
  assert.deepEqual(graphUnconfirmed(pages, "new"), []);
});
