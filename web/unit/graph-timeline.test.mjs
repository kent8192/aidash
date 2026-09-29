import assert from "node:assert/strict";
import test from "node:test";
import { mergeGraphTimeline } from "../src/collaboration/graph-timeline.ts";

const now = Date.parse("2026-09-29T12:00:00Z");
const at = (hours) => new Date(now - hours * 3_600_000).toISOString();
const key = (peer, id = "same-run") =>
  JSON.stringify(["resource", peer, "run", id]);
const node = (peer) => ({ id: key(peer), nodeId: peer });
const marker = (peer, age = 1) => ({
  kind: "run.phase",
  at: at(age),
  reference: key(peer),
});
const page = (peer, activity = [marker(peer)]) => ({
  node_id: peer,
  nodes: [{ id: key(peer), node_id: peer }],
  activity,
});
const local = [
  {
    id: "local-event",
    kind: "task.created",
    created_at: at(2),
    reference: key("aidash://a"),
  },
];

test("timeline includes current peer activity in chronological order and count", () => {
  const peer = "aidash://b";
  const events = mergeGraphTimeline(
    local,
    new Map([[peer, page(peer)]]),
    [node(peer)],
    24,
    now,
  );
  assert.equal(events.length, 2);
  assert.deepEqual(events.map((event) => event.created_at), [at(2), at(1)]);
  assert.equal(events[1].reference, key(peer));
});

test("identical remote resource UUIDs select the correct node-qualified endpoint", () => {
  const b = "aidash://b";
  const c = "aidash://c";
  const pages = new Map([
    [b, page(b)],
    [c, page(c)],
  ]);
  const events = mergeGraphTimeline([], pages, [node(b), node(c)], 24, now);
  assert.equal(events.length, 2);
  assert.equal(new Set(events.map((event) => event.id)).size, 2);
  assert.deepEqual(
    new Set(events.map((event) => event.reference)),
    new Set([key(b), key(c)]),
  );
});

test("removing expired, revoked, collapsed or stale-scope pages removes their events", () => {
  const b = "aidash://b";
  assert.equal(
    mergeGraphTimeline(local, new Map([[b, page(b)]]), [node(b)], 24, now).length,
    2,
  );
  assert.deepEqual(mergeGraphTimeline(local, new Map(), [node(b)], 24, now), local);
});

test("markers must belong to both the page and the current merged graph", () => {
  const b = "aidash://b";
  const c = "aidash://c";
  const malformed = page(b, [marker(c)]);
  assert.deepEqual(
    mergeGraphTimeline([], new Map([[b, malformed]]), [node(c)], 24, now),
    [],
  );
  assert.deepEqual(
    mergeGraphTimeline([], new Map([[b, page(b)]]), [], 24, now),
    [],
  );
  assert.deepEqual(
    mergeGraphTimeline([], new Map([[b, page(c)]]), [node(c)], 24, now),
    [],
  );
});

test("the selected time window also applies to peer activity", () => {
  const b = "aidash://b";
  const remote = page(b, [marker(b, 48), marker(b)]);
  const pages = new Map([[b, remote]]);
  assert.equal(mergeGraphTimeline([], pages, [node(b)], 24, now).length, 1);
  assert.equal(mergeGraphTimeline([], pages, [node(b)], 0, now).length, 2);
});

test("invalid and future timestamps never break timeline positioning", () => {
  const b = "aidash://b";
  const remote = page(b, [
    { ...marker(b), at: "invalid" },
    marker(b, -1),
    marker(b),
  ]);
  assert.equal(
    mergeGraphTimeline([], new Map([[b, remote]]), [node(b)], 0, now).length,
    1,
  );
});
