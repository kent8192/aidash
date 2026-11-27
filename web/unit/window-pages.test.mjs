import { test } from "node:test";
import assert from "node:assert/strict";
import {
  continuation,
  emptyPages,
  landPage,
  mergeEventWindow,
  mergeWindow,
  olderRows,
  rereadPages,
  uniqueById,
} from "../src/window-pages.ts";
import { threadStatus } from "../src/shell/thread-status.ts";

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

const task = (id, thread_id, status) => ({ id, thread_id, status });
const request = (thread_id, response = null) => ({ thread_id, response });

test("a complete Task window shows done, idle and counts", () => {
  const complete = { moreTasks: false, moreRequests: false };
  const tasks = [task("t1", "w1", "COMPLETED"), task("t2", "w1", "COMPLETED")];
  assert.deepEqual(threadStatus(tasks, [], "w1", complete), {
    tone: "done",
    progress: { completed: 2, total: 2 },
    pending: 0,
    pendingMore: false,
  });
  assert.equal(threadStatus(tasks, [], "w2", complete).tone, "idle");
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
  assert.deepEqual(threadStatus(tasks, requests, "w1", truncated), {
    tone: "unknown",
    progress: null,
    pending: 1,
    pendingMore: true,
  });
  assert.equal(threadStatus(tasks, [], "w2", truncated).tone, "unknown");
  assert.equal(threadStatus(tasks, [], "w3", truncated).tone, "attention");
  assert.equal(threadStatus(tasks, [], "w4", truncated).tone, "active");
});

// Rows w1 (oldest) .. w9; pages of two, each continuing after its last id.
const chain = ["w9", "w8", "w7", "w6", "w5", "w4", "w3", "w2", "w1"];
function pageAfter(cursor) {
  const start = chain.indexOf(cursor) + 1;
  const items = chain.slice(start, start + 2).map((id) => ({ id }));
  return {
    items,
    next_cursor: start + 2 < chain.length ? items[items.length - 1].id : null,
  };
}

test("a moved window boundary re-reads the loaded range without a gap", async () => {
  // The window held w9..w8 and one page (w7, w6) was loaded. A new Thread
  // w10 moves the boundary to w9: re-reading from it brings w8 back.
  const result = await rereadPages(
    async (cursor) => pageAfter(cursor),
    "w9",
    () => 1,
  );
  assert.deepEqual(result, {
    items: [{ id: "w8" }, { id: "w7" }],
    read: 1,
    cursor: "w7",
  });
});

test("a re-read covers a page asked for while it runs", async () => {
  let depth = 1;
  const result = await rereadPages(
    async (cursor) => {
      // "Show more" is clicked while the first page is in flight.
      depth = 2;
      return pageAfter(cursor);
    },
    "w9",
    () => depth,
  );
  assert.equal(result.read, 2);
  assert.equal(result.cursor, "w5");
  assert.deepEqual(
    result.items.map((row) => row.id),
    ["w8", "w7", "w6", "w5"],
  );
});

test("a re-read stops where the list ends", async () => {
  const result = await rereadPages(
    async (cursor) => pageAfter(cursor),
    "w3",
    () => 5,
  );
  assert.deepEqual(result, {
    items: [{ id: "w2" }, { id: "w1" }],
    read: 1,
    cursor: null,
  });
  assert.deepEqual(
    await rereadPages(
      async () => assert.fail("no read"),
      null,
      () => 5,
    ),
    { items: [], read: 0, cursor: null },
  );
});

test("a show-more page lands only on the pages it continued", () => {
  const pages = {
    ...emptyPages("node-a"),
    items: [{ id: "w7" }],
    read: 1,
    cursor: "w7",
    loading: true,
  };
  const page = { items: [{ id: "w6" }], next_cursor: "w6" };
  assert.deepEqual(landPage(pages, "node-a", 1, "w7", page), {
    ...pages,
    items: [{ id: "w7" }, { id: "w6" }],
    read: 2,
    cursor: "w6",
    loading: false,
  });
  // A re-read replaced the pages while it was in flight (and read it too).
  const reread = {
    ...pages,
    items: [{ id: "w8" }, { id: "w7" }],
    read: 2,
    cursor: "w6",
  };
  assert.deepEqual(landPage(reread, "node-a", 1, "w7", page), {
    ...reread,
    loading: false,
  });
  // Same depth, different chain.
  const moved = { ...pages, cursor: "w8" };
  assert.equal(landPage(moved, "node-a", 1, "w7", page).items, moved.items);
  // Another Node's pages are left alone.
  assert.equal(landPage(pages, "node-b", 1, "w7", page), pages);
});

test("a fresh event wins overlaps and older pages precede it by sequence", () => {
  const fresh = [{ id: "new", sequence: 8, kind: "fresh" }];
  const older = [
    { id: "new", sequence: 8, kind: "stale" },
    { id: "oldest", sequence: 2, kind: "older" },
    { id: "middle", sequence: 5, kind: "older" },
  ];
  assert.deepEqual(mergeEventWindow(fresh, older), [
    older[1],
    older[2],
    fresh[0],
  ]);
});
