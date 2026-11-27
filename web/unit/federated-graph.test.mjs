import assert from "node:assert/strict";
import test from "node:test";
import { bundleForTest } from "./bundle-for-test.mjs";

const { validateGraphPage } = await bundleForTest(import.meta.url, {
  entryPoints: ["src/collaboration/federated-graph.ts"],
});

test("peer activity continuations must agree with the cursor and reference this page", () => {
  const peer = "aidash://graph-b";
  const id = JSON.stringify(["resource", peer, "task", "task-b"]);
  const at = "2026-11-27T00:00:00Z";
  const page = {
    node_id: peer,
    generation: "generation",
    checked_at: at,
    nodes: [
      {
        id,
        node_id: peer,
        kind: "task",
        name: { en: "B Task" },
        resource_id: "task-b",
        version: null,
        goal_body: null,
      },
    ],
    edges: [],
    activity: [{ kind: "task.started", at, reference: id }],
    activity_cursor: "older",
    activity_more: true,
    next_cursor: null,
  };
  assert.equal(validateGraphPage(page, peer), true);
  assert.equal(validateGraphPage({ ...page, activity: [] }, peer), true);
  assert.equal(
    validateGraphPage({ ...page, activity_more: false }, peer),
    false,
  );
  assert.equal(
    validateGraphPage({ ...page, activity_cursor: null }, peer),
    false,
  );
  assert.equal(
    validateGraphPage(
      {
        ...page,
        activity: [{ ...page.activity[0], reference: "foreign-task" }],
      },
      peer,
    ),
    false,
  );
});
