import { test } from "node:test";
import assert from "node:assert/strict";
import {
  citationParts,
  linkable,
} from "../src/collaboration/citations-model.ts";

const first = "ev_0b9f6a52-3c1e-4c2a-9d7e-5f0c1a2b3c4d";
const second = "ev_1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f";

test("citations are numbered by first use and keep their number when repeated", () => {
  const parts = citationParts(
    `Alpha [[cite:${first}]] beta [[cite:${second}]] gamma [[cite:${first}]].`,
  );
  assert.deepEqual(parts, [
    { kind: "text", text: "Alpha " },
    { kind: "citation", reference: first, ordinal: 1 },
    { kind: "text", text: " beta " },
    { kind: "citation", reference: second, ordinal: 2 },
    { kind: "text", text: " gamma " },
    { kind: "citation", reference: first, ordinal: 1 },
    { kind: "text", text: "." },
  ]);
});

test("references are normalized to the server's lowercase form", () => {
  const parts = citationParts(
    `[[cite:${first.toUpperCase().replace("EV_", "ev_")}]]`,
  );
  assert.deepEqual(parts, [{ kind: "citation", reference: first, ordinal: 1 }]);
});

test("malformed or unterminated markers stay literal text", () => {
  for (const text of [
    "[[cite:ev_nope]]",
    `[[cite:${first}]`,
    `[[cite:EV_${first.slice(3)}]]`,
    `[[cite: ${first}]]`,
    "no citations at all",
  ])
    assert.deepEqual(citationParts(text), [{ kind: "text", text }], text);
  assert.deepEqual(citationParts(""), []);
});

test("only http(s) addresses become links", () => {
  assert.equal(linkable("https://example.com/a"), true);
  assert.equal(linkable("HTTP://example.com"), true);
  assert.equal(linkable("javascript:alert(1)"), false);
  assert.equal(linkable("//example.com"), false);
});
