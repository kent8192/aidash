import assert from "node:assert/strict";
import test from "node:test";
import {
  graphEmptyText,
  graphUnconfirmed,
} from "../src/collaboration/graph-unconfirmed.ts";

test("graph warning counts the current pages under their answering Nodes", () => {
  const row = (count) => ({
    node: "aidash://b",
    kind: "task",
    thread_id: "w",
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

test("an empty page whose rows another Node could not confirm shows no empty text", () => {
  const counted = {
    nodes: [],
    next_cursor: null,
    unconfirmed: [
      { node: "aidash://b", kind: "task", thread_id: "w", count: 4 },
    ],
  };
  assert.equal(graphEmptyText(counted), null);
  assert.equal(graphEmptyText({ ...counted, next_cursor: "c" }), null);
  assert.equal(
    graphEmptyText({ nodes: [], next_cursor: null, unconfirmed: [] }),
    "view",
  );
  assert.equal(graphEmptyText({ nodes: [], next_cursor: "c" }), "page");
});
