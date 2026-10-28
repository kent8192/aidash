import test from "node:test";
import assert from "node:assert/strict";
import {
  mergeMessagePages,
  submissionFor,
  validAttachments,
  validRunMediaAttachments,
  runMediaAccept,
  validRunMediaRoute,
} from "../src/collaboration/conversation-model.ts";
import {
  MAX_FILE_CITATIONS,
  addFileCitation,
  fileCitationFrom,
  fileCitationPayload,
  removeFileCitation,
} from "../src/collaboration/file-citation-model.ts";

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

test("message retry identity includes attachment order", () => {
  let n = 0;
  const key = () => String(++n);
  const first = submissionFor(null, "workspace", null, "source", key, [
    "b",
    "a",
  ]);
  assert.notEqual(
    submissionFor(first, "workspace", null, "source", key, ["a", "b"]).key,
    first.key,
  );
  assert.equal(
    submissionFor(first, "workspace", null, "source", key, ["b", "a"]).key,
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

test("run media selection accepts only supported image and audio MIME types", () => {
  const file = { name: "evidence.png", size: 1024, type: "image/png" };
  assert.equal(validRunMediaAttachments([file]), true);
  assert.equal(
    validRunMediaAttachments([
      { ...file, name: "sound.wav", type: "audio/wav" },
    ]),
    true,
  );
  for (const type of ["", "application/pdf", "text/plain", "image/svg+xml"]) {
    assert.equal(validRunMediaAttachments([{ ...file, type }]), false);
  }
});

test("run media routes require one current route for every selected format", () => {
  const routes = [["image/png"], ["audio/wav", "audio/x-wav"]];
  assert.equal(runMediaAccept(routes), "image/png,audio/wav,audio/x-wav");
  assert.equal(validRunMediaRoute([{ type: "image/png" }], routes), true);
  assert.equal(validRunMediaRoute([{ type: "audio/x-wav" }], routes), true);
  assert.equal(
    validRunMediaRoute([{ type: "image/png" }, { type: "audio/wav" }], routes),
    false,
  );
  assert.equal(validRunMediaRoute([{ type: "image/png" }], []), false);
  assert.equal(runMediaAccept([["application/pdf"]]), "");
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

test("message retry identity includes cited File versions", () => {
  let n = 0;
  const key = () => String(++n);
  const first = submissionFor(
    null,
    "workspace",
    null,
    "source",
    key,
    [],
    ["area:one"],
  );
  assert.equal(
    submissionFor(first, "workspace", null, "source", key, [], ["area:one"])
      .key,
    first.key,
  );
  assert.notEqual(
    submissionFor(first, "workspace", null, "source", key, [], ["area:two"])
      .key,
    first.key,
  );
  assert.notEqual(
    submissionFor(first, "workspace", null, "source", key, [], []).key,
    first.key,
  );
});

test("citation chips dedupe, cap at eight and map to the request payload", () => {
  const citation = (index) => ({
    key: `area:${index}`,
    areaId: "area",
    fileId: String(index),
    path: `notes/${index}.md`,
  });
  let chips = [];
  for (let index = 0; index < MAX_FILE_CITATIONS + 2; index += 1)
    chips = addFileCitation(chips, citation(index));
  assert.equal(chips.length, MAX_FILE_CITATIONS);
  assert.equal(addFileCitation(chips, citation(0)), chips);
  assert.deepEqual(fileCitationPayload(chips).slice(0, 2), [
    { area_id: "area", file_id: "0" },
    { area_id: "area", file_id: "1" },
  ]);
  assert.deepEqual(
    removeFileCitation(chips, "area:0").map((entry) => entry.fileId),
    ["1", "2", "3", "4", "5", "6", "7"],
  );
});

test("a citation keeps the path and short digest the composer shows", () => {
  const citation = fileCitationFrom(
    "area",
    {
      file_id: "file",
      path: "reports/summary.md",
      digest: "a".repeat(64),
      size: 42,
      media_type: "text/markdown",
    },
    7,
  );
  assert.equal(citation.key, "area:file");
  assert.equal(citation.path, "reports/summary.md");
  assert.equal(citation.digest.slice(0, 8), "aaaaaaaa");
  assert.equal(citation.revision, 7);
});

test("a remote citation pins its source Run in the message payload", () => {
  const citation = fileCitationFrom(
    "remote-area",
    { file_id: "version", path: "page.html" },
    4,
    "source-run",
  );
  assert.equal(citation.key, "source-run:remote-area:version");
  assert.deepEqual(fileCitationPayload([citation]), [
    { area_id: "remote-area", file_id: "version", run_id: "source-run" },
  ]);
});
