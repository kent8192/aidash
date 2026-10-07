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
for (const kind of ["completed", "failed"]) {
  test(`${kind} revisions retain history, queued toast and unread identity`, () => {
    const store = new NoticeStore();
    const request = entry("request", "input");
    store.observe(frame([entry("task")]));
    store.observe(frame([entry("task"), request]));
    store.takeBatch(0);
    store.markRead();
    store.observe(frame([entry("task", kind, "home:one", "1"), request]));
    const id = store.getSnapshot().recent[0].id;
    const updated = {
      ...entry("task", kind, "home:one", "2"),
      detail: "Updated task title",
    };
    store.observe(frame([updated, request]));
    assert.equal(store.getSnapshot().recent[0].id, id);
    assert.equal(store.getSnapshot().recent[0].detail, updated.detail);
    assert.equal(store.getSnapshot().unread, true);
    assert.equal(store.getCurrent(id).signature, "2");
    assert.deepEqual(store.takeBatch(1), []);
    assert.deepEqual(store.takeBatch(NOTICE_INTERVAL), [store.getCurrent(id)]);
    store.markRead(id);
    store.observe(frame([entry("task", kind, "home:one", "3"), request]));
    assert.equal(store.getSnapshot().unread, false);
    assert.deepEqual(store.takeBatch(2 * NOTICE_INTERVAL), []);
    store.observe(frame([request]));
    assert.equal(store.getSnapshot().recent.length, 0);
    assert.equal(store.getCurrent(id), undefined);
  });
  test(`${kind} history and throttled requests survive temporary snapshot outages`, () => {
    const store = new NoticeStore();
    const first = entry("first", "approval");
    store.observe(frame([entry("task")]));
    store.observe(frame([entry("task"), first]));
    store.takeBatch(0);
    store.markRead();
    const restored = frame([
      entry("task", kind),
      first,
      entry("queued", "input"),
    ]);
    store.observe(restored);
    const snapshot = store.getSnapshot();
    const completed = snapshot.recent[0];
    const queued = snapshot.pending.find((item) => item.target.id === "queued");
    const delay = store.delay(100);
    store.observe(null);
    store.observe(null);
    assert.equal(store.getSnapshot(), snapshot);
    assert.equal(store.getSnapshot().unread, true);
    assert.equal(store.getCurrent(completed.id), completed);
    assert.equal(store.getCurrent(queued.id), queued);
    assert.equal(store.delay(100), delay);
    store.observe(restored);
    assert.deepEqual(store.getSnapshot(), snapshot);
    assert.deepEqual(store.takeBatch(100), []);
    assert.deepEqual(
      store.takeBatch(NOTICE_INTERVAL).map((item) => item.id),
      [completed.id, queued.id],
    );
    store.observe(frame([], []));
    assert.deepEqual(store.getSnapshot(), {
      pending: [],
      recent: [],
      unread: false,
    });
    assert.equal(store.getCurrent(completed.id), undefined);
    assert.equal(store.getCurrent(queued.id), undefined);
  });
}
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
  const tasks = Array.from({ length: 75 }, (_, index) =>
    entry(`task-${index}`),
  );
  store.observe(frame(tasks));
  const completed = tasks.map((task) => ({ ...task, kind: "completed" }));
  store.observe(frame(completed));
  const history = store.getSnapshot().recent;
  assert.equal(history.length, NOTICE_HISTORY_LIMIT);
  const requests = Array.from({ length: 75 }, (_, index) =>
    entry(`request-${index}`, "input"),
  );
  store.observe(frame([...completed, ...requests]));
  assert.deepEqual(store.getSnapshot().recent, history);
  assert.equal(store.getSnapshot().pending.length, 75);
  assert.equal(store.takeBatch(100).length, 150);
});
