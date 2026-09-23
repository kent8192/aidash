import test from "node:test";
import assert from "node:assert/strict";
import {
  mergeMessagePages,
  submissionFor,
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
    mergeMessagePages(pages).map((item) => [item.message.id, item.message.content]),
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
