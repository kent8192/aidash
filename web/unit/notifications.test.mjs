import { test } from "node:test";
import assert from "node:assert/strict";
import {
  NoticeStore,
  NOTICE_INTERVAL,
  NOTICE_HISTORY_LIMIT,
} from "../src/notifications/model.ts";

function entry(id, kind = null, scope = "home:one", signature = String(kind)) {
  return {
    key: `${scope}:${id}`,
    scope,
    signature,
    kind,
    detail: id,
    workspaceTitle: scope,
    target: {
      kind: ["input", "approval"].includes(kind) ? "human" : "task",
      node: scope.split(":")[0],
      id,
      workspace: scope.split(":")[1],
    },
  };
}
function frame(entries, scopes = ["home:one"]) {
  return { entries, scopes };
}

test("initial pending requests remain reachable without announcing history", () => {
  const store = new NoticeStore();
  store.observe(frame([entry("old", "input"), entry("done", "completed")]));
  assert.equal(store.getSnapshot().pending.length, 1);
  assert.deepEqual(store.takeBatch(100), []);
  assert.equal(store.getSnapshot().recent.length, 0);
});
test("new inputs deduplicate across polls, missing rows and reconnects", () => {
  const store = new NoticeStore();
  store.observe(frame([]));
  store.observe(frame([entry("new", "approval")]));
  assert.equal(store.takeBatch(100).length, 1);
  store.observe(frame([]));
  store.observe(null);
  store.observe(frame([entry("new", "approval")]));
  assert.deepEqual(store.takeBatch(100 + NOTICE_INTERVAL), []);
});
test("only observed terminal transitions announce, including failure recovery", () => {
  const store = new NoticeStore();
  store.observe(frame([entry("task", null, "home:one", "RUNNING")]));
  store.observe(
    frame([
      entry("task", null, "home:one", "THINKING"),
      entry("loaded-history", "completed"),
    ]),
  );
  assert.deepEqual(store.takeBatch(0), []);
  store.observe(frame([entry("task", "failed")]));
  assert.equal(store.takeBatch(0)[0].kind, "failed");
  store.observe(
    frame([entry("task", "failed", "home:one", "revision-changed")]),
  );
  assert.deepEqual(store.takeBatch(NOTICE_INTERVAL), []);
  store.observe(frame([entry("task", "completed")]));
  assert.equal(store.takeBatch(NOTICE_INTERVAL)[0].kind, "completed");
});
test("bursts are grouped and a second announcement waits for the global interval", () => {
  const store = new NoticeStore();
  store.observe(frame([]));
  store.observe(frame([entry("one", "input"), entry("two", "approval")]));
  assert.equal(store.takeBatch(10).length, 2);
  store.observe(
    frame([
      entry("one", "input"),
      entry("two", "approval"),
      entry("three", "input"),
    ]),
  );
  assert.deepEqual(store.takeBatch(100), []);
  assert.equal(store.delay(100), NOTICE_INTERVAL - 90);
  assert.equal(store.takeBatch(10 + NOTICE_INTERVAL).length, 1);
});
test("resolved and revoked resources are removed before a queued toast can open them", () => {
  const store = new NoticeStore();
  store.observe(frame([]));
  store.observe(frame([entry("new", "input")]));
  const id = store.getSnapshot().pending[0].id;
  store.observe(frame([entry("new", null)]));
  assert.equal(store.getCurrent(id), undefined);
  assert.deepEqual(store.takeBatch(100), []);
  assert.equal(store.getSnapshot().pending.length, 0);
  store.observe(frame([entry("later", "approval")]));
  store.observe(frame([], []));
  assert.deepEqual(store.takeBatch(100), []);
  assert.deepEqual(store.getSnapshot(), {
    pending: [],
    recent: [],
    unread: false,
  });
});
test("newly granted and re-granted scopes seed quietly", () => {
  const store = new NoticeStore();
  store.observe(frame([]));
  const extra = entry("old", "input", "remote:two");
  store.observe(frame([extra], ["home:one", "remote:two"]));
  assert.deepEqual(store.takeBatch(100), []);
  assert.equal(store.getSnapshot().pending.length, 1);
  store.observe(frame([]));
  store.observe(frame([extra], ["home:one", "remote:two"]));
  assert.deepEqual(store.takeBatch(100), []);
});
test("opening the center clears unread state and cancels redundant queued announcements", () => {
  const store = new NoticeStore();
  store.observe(frame([]));
  store.observe(frame([entry("new", "input")]));
  assert.equal(store.getSnapshot().unread, true);
  store.markRead();
  assert.equal(store.getSnapshot().unread, false);
  assert.equal(store.getSnapshot().pending.length, 1);
  assert.deepEqual(store.takeBatch(100), []);
});
test("visiting one notice acknowledges it while retaining other queued updates", () => {
  const store = new NoticeStore();
  store.observe(frame([entry("task", null)]));
  store.observe(
    frame([entry("task", "completed"), entry("question", "input")]),
  );
  const completed = store
    .getSnapshot()
    .recent.find((item) => item.kind === "completed");
  store.markRead(completed.id);
  assert.equal(store.getSnapshot().unread, true);
  assert.deepEqual(
    store.takeBatch(100).map((item) => item.key),
    ["home:one:question"],
  );
  store.markRead(store.getSnapshot().pending[0].id);
  assert.equal(store.getSnapshot().unread, false);
  assert.equal(store.getSnapshot().pending.length, 1);
});
test("node identity keeps colliding request IDs distinct", () => {
  const store = new NoticeStore();
  const scopes = ["home:one", "remote:one"];
  store.observe(frame([], scopes));
  store.observe(
    frame(
      [entry("same", "input"), entry("same", "input", "remote:one")],
      scopes,
    ),
  );
  const batch = store.takeBatch(100);
  assert.equal(batch.length, 2);
  assert.notEqual(batch[0].id, batch[1].id);
  assert.deepEqual(batch.map((item) => item.target.node).sort(), [
    "home",
    "remote",
  ]);
});
test("history is bounded while every pending request remains reachable", () => {
  const store = new NoticeStore();
  store.observe(frame([]));
  const entries = Array.from({ length: 75 }, (_, index) =>
    entry(`request-${index}`, "input"),
  );
  store.observe(frame(entries));
  assert.equal(store.getSnapshot().recent.length, NOTICE_HISTORY_LIMIT);
  assert.equal(store.getSnapshot().pending.length, 75);
  assert.equal(store.takeBatch(100).length, 75);
});
