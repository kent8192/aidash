import { test } from "node:test";
import assert from "node:assert/strict";
import {
  continuation,
  emptyPages,
  mergeWindow,
  olderRows,
  uniqueById,
} from "../src/window-pages.ts";
import { workspaceStatus } from "../src/shell/workspace-status.ts";

test("a fresh window keeps loaded older rows and wins for shared ids", () => {
  const fresh = [
    { id: "w3", title: "new" },
    { id: "w2", title: "renamed" },
  ];
  const older = [
    { id: "w2", title: "old" },
    { id: "w1", title: "older" },
  ];
  assert.deepEqual(mergeWindow(fresh, older), [
    { id: "w3", title: "new" },
    { id: "w2", title: "renamed" },
    { id: "w1", title: "older" },
  ]);
});

test("a Run repeated across pages is listed once, as last read", () => {
  assert.deepEqual(
    uniqueById([
      { id: "r1", status: "RUNNING" },
      { id: "r2", status: "RUNNING" },
      { id: "r1", status: "COMPLETED" },
    ]),
    [
      { id: "r1", status: "COMPLETED" },
      { id: "r2", status: "RUNNING" },
    ],
  );
});

test("the advanced cursor survives a refresh until the window holds everything", () => {
  const pages = { ...emptyPages("ctx:node"), read: 1, cursor: "page-2" };
  assert.equal(continuation(emptyPages("ctx:node"), "window"), "window");
  assert.equal(continuation(pages, "window-after-refresh"), "page-2");
  assert.equal(continuation({ ...pages, cursor: null }, "window"), null);
  assert.equal(continuation(pages, null), null);
  const loaded = { ...pages, items: [{ id: "h1" }] };
  assert.deepEqual(olderRows(loaded, "window"), [{ id: "h1" }]);
  // A complete pending window no longer lists an answered request.
  assert.equal(olderRows(loaded, null), null);
});

const task = (id, workspace_id, status) => ({ id, workspace_id, status });
const request = (workspace_id, response = null) => ({ workspace_id, response });

test("a complete Task window shows done, idle and counts", () => {
  const complete = { moreTasks: false, moreRequests: false };
  const tasks = [task("t1", "w1", "COMPLETED"), task("t2", "w1", "COMPLETED")];
  assert.deepEqual(workspaceStatus(tasks, [], "w1", complete), {
    tone: "done",
    progress: { completed: 2, total: 2 },
    pending: 0,
    pendingMore: false,
  });
  assert.equal(workspaceStatus(tasks, [], "w2", complete).tone, "idle");
});

test("a truncated Task window claims neither done nor idle", () => {
  const truncated = { moreTasks: true, moreRequests: true };
  const tasks = [
    task("t1", "w1", "COMPLETED"),
    task("t2", "w1", "COMPLETED"),
    task("t3", "w3", "FAILED"),
    task("t4", "w4", "RUNNING"),
  ];
  const requests = [request("w1"), request("w1", { approved: true })];
  assert.deepEqual(workspaceStatus(tasks, requests, "w1", truncated), {
    tone: "unknown",
    progress: null,
    pending: 1,
    pendingMore: true,
  });
  assert.equal(workspaceStatus(tasks, [], "w2", truncated).tone, "unknown");
  assert.equal(workspaceStatus(tasks, [], "w3", truncated).tone, "attention");
  assert.equal(workspaceStatus(tasks, [], "w4", truncated).tone, "active");
});
