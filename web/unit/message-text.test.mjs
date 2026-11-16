import { test } from "node:test";
import assert from "node:assert/strict";
import { messageParts } from "../src/collaboration/message-text-model.ts";

test("inline composer syntax becomes spans, other text stays literal", () => {
  assert.deepEqual(messageParts("a **b** _c_ `d` @bob snake_case e"), [
    { kind: "text", text: "a " },
    { kind: "strong", text: "b" },
    { kind: "text", text: " " },
    { kind: "em", text: "c" },
    { kind: "text", text: " " },
    { kind: "code", text: "d" },
    { kind: "text", text: " " },
    { kind: "mention", text: "@bob" },
    { kind: "text", text: " snake_case e" },
  ]);
});

test("a citation marker is ordinary text", () => {
  const marker = "[[cite:ev_00000000-0000-0000-0000-000000000000]]";
  assert.deepEqual(messageParts(`see ${marker}`), [
    { kind: "text", text: `see ${marker}` },
  ]);
});
