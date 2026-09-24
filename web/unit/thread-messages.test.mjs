import test from "node:test";
import assert from "node:assert/strict";
import {
  mergeMessagePages,
  submissionFor,
  validAttachments,
} from "../src/collaboration/conversation-model.ts";

const message = (id, content = id) => ({
  message: { id, content, created_at: `2026-09-23T00:00:0${id}Z` },
  thread_id: null,
});

test("history is chronological and refreshed messages win over older pages", () => {
  const pages = [
    { messages: [message("2", "new"), message("3")] },
    { messages: [message("1"), message("2", "old")] },
  ];
  assert.deepEqual(
    mergeMessagePages(pages).map((item) => [
      item.message.id,
      item.message.content,
    ]),
    [
      ["1", "1"],
      ["2", "new"],
      ["3", "3"],
    ],
  );
});

test("identical retry keeps its key and a different thread cannot reuse it", () => {
  let n = 0;
  const key = () => String(++n);
  const first = submissionFor(null, "channel", null, "  hello  ", key);
  assert.equal(first.content, "hello");
  assert.equal(
    submissionFor(first, "channel", null, "hello", key).key,
    first.key,
  );
  for (const [workspace, thread, content] of [
    ["channel", "thread", "hello"],
    ["other-channel", null, "hello"],
    ["channel", null, "changed"],
  ]) {
    assert.notEqual(
      submissionFor(first, workspace, thread, content, key).key,
      first.key,
    );
  }
});

test("empty page list remains empty without inventing conversation data", () => {
  assert.deepEqual(mergeMessagePages([]), []);
});

test("message retry identity includes attachment membership", () => {
  let n = 0;
  const key = () => String(++n);
  const first = submissionFor(null, "workspace", null, "source", key, [
    "b",
    "a",
  ]);
  assert.equal(
    submissionFor(first, "workspace", null, "source", key, ["a", "b"]).key,
    first.key,
  );
  assert.notEqual(
    submissionFor(first, "workspace", null, "source", key, ["b"]).key,
    first.key,
  );
  assert.notEqual(
    submissionFor(first, "workspace", null, "source", key, []).key,
    first.key,
  );
});

test("attachment validation rejects empty, oversized, unsafe and over-count selections", () => {
  const file = { name: "evidence.txt", size: 1024 * 1024 };
  assert.equal(validAttachments([file]), true);
  for (const bad of [
    { ...file, size: 0 },
    { ...file, size: file.size + 1 },
    { ...file, name: "" },
    { ...file, name: "../secret.txt" },
    { ...file, name: "a\\b" },
    { ...file, name: "bad\nname" },
    { ...file, name: "語".repeat(86) },
  ]) {
    assert.equal(validAttachments([bad]), false);
  }
  assert.equal(validAttachments(Array(9).fill(file)), false);
});

test("participants keep node and version identity and prefer a still-active run", async () => {
  const { channelAgents } = await import(
    "../src/collaboration/workspace-model.ts"
  );
  const located = (node, id, phase, updated_at, version = "1.0.0") => ({
    node,
    run: {
      id,
      agent_id: "researcher",
      agent_version: version,
      phase,
      updated_at,
    },
  });
  const runs = [
    located("home", "recent-done", "COMPLETED", "2026-09-24T12:00:00Z"),
    located("home", "active", "THINKING", "2026-09-24T11:00:00Z"),
    located("peer", "remote", "COMPLETED", "2026-09-24T10:00:00Z"),
    located(
      "home",
      "new-version",
      "COMPLETED",
      "2026-09-24T09:00:00Z",
      "2.0.0",
    ),
  ];
  assert.deepEqual(
    channelAgents(runs).map((item) => item.run.id),
    ["active", "remote", "new-version"],
  );
  assert.equal(runs[0].run.id, "recent-done");
});
