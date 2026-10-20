import { test } from "node:test";
import assert from "node:assert/strict";
import {
  replayAfterHint,
  replayAfterQueryChange,
  sameQuery,
} from "../src/mcp-hint.ts";

const asked = {
  transport: "mcp",
  endpoint: "https://example.com/mcp",
  toolName: "search",
};

test("a hint answer belongs only to the transport, endpoint and tool it was read for", () => {
  assert.equal(
    sameQuery(asked, asked.transport, asked.endpoint, asked.toolName),
    true,
  );
  assert.equal(sameQuery(asked, "http", asked.endpoint, asked.toolName), false);
  assert.equal(
    sameQuery(
      asked,
      asked.transport,
      "https://other.example/mcp",
      asked.toolName,
    ),
    false,
  );
  assert.equal(
    sameQuery(asked, asked.transport, asked.endpoint, "search-more"),
    false,
  );
});

test("a read-only hint prefills the choice and a later answer undoes the prefill", () => {
  const prefilled = replayAfterHint("unsafe", true, false);
  assert.deepEqual(prefilled, { replay: "read_only", prefilled: true });
  assert.deepEqual(replayAfterHint("read_only", false, prefilled.prefilled), {
    replay: "unsafe",
    prefilled: false,
  });
  assert.deepEqual(replayAfterHint("unsafe", null, false), {
    replay: "unsafe",
    prefilled: false,
  });
});

test("a person's own read-only choice is never undone by a hint", () => {
  assert.deepEqual(replayAfterHint("read_only", null, false), {
    replay: "read_only",
    prefilled: false,
  });
  assert.deepEqual(replayAfterHint("read_only", false, false), {
    replay: "read_only",
    prefilled: false,
  });
});

test("editing the queried endpoint or tool undoes only an automatic prefill", () => {
  assert.equal(replayAfterQueryChange("read_only", true), "unsafe");
  assert.equal(replayAfterQueryChange("read_only", false), "read_only");
  assert.equal(replayAfterQueryChange("unsafe", true), "unsafe");
});
