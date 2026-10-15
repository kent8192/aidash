import { test } from "node:test";
import assert from "node:assert/strict";
import {
  citationParts,
  linkable,
  messageParts,
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

test("a citation inside bold, italics or code keeps the surrounding span", () => {
  assert.deepEqual(
    messageParts(`**important [[cite:${first}]] claim**`, true),
    [
      {
        kind: "strong",
        parts: [
          { kind: "text", text: "important " },
          { kind: "citation", reference: first, ordinal: 1 },
          { kind: "text", text: " claim" },
        ],
      },
    ],
  );
  // The underscore in `ev_` neither opens nor closes italics.
  assert.deepEqual(messageParts(`_aside [[cite:${first}]] note_`, true), [
    {
      kind: "em",
      parts: [
        { kind: "text", text: "aside " },
        { kind: "citation", reference: first, ordinal: 1 },
        { kind: "text", text: " note" },
      ],
    },
  ]);
  assert.deepEqual(messageParts(`snake_case [[cite:${first}]]`, true), [
    { kind: "text", text: "snake_case " },
    { kind: "citation", reference: first, ordinal: 1 },
  ]);
  assert.deepEqual(messageParts(`\`[[cite:${first}]]\` @bob`, true), [
    {
      kind: "code",
      parts: [{ kind: "citation", reference: first, ordinal: 1 }],
    },
    { kind: "text", text: " " },
    { kind: "mention", text: "@bob" },
  ]);
});

test("numbering by first use continues across formatted spans", () => {
  assert.deepEqual(
    messageParts(
      `[[cite:${second}]] **[[cite:${first}]] and [[cite:${second}]]** [[cite:${first}]]`,
      true,
    ),
    [
      { kind: "citation", reference: second, ordinal: 1 },
      { kind: "text", text: " " },
      {
        kind: "strong",
        parts: [
          { kind: "citation", reference: first, ordinal: 2 },
          { kind: "text", text: " and " },
          { kind: "citation", reference: second, ordinal: 1 },
        ],
      },
      { kind: "text", text: " " },
      { kind: "citation", reference: first, ordinal: 2 },
    ],
  );
});

test("markers in text no Run published stay literal, formatting still applies", () => {
  assert.deepEqual(messageParts(`**see [[cite:${first}]]**`, false), [
    {
      kind: "strong",
      parts: [{ kind: "text", text: `see [[cite:${first}]]` }],
    },
  ]);
  assert.deepEqual(messageParts(`Pasted [[cite:${first}]].`, false), [
    { kind: "text", text: "Pasted " },
    { kind: "text", text: `[[cite:${first}]]` },
    { kind: "text", text: "." },
  ]);
});

test("only http(s) addresses become links", () => {
  assert.equal(linkable("https://example.com/a"), true);
  assert.equal(linkable("HTTP://example.com"), true);
  assert.equal(linkable("javascript:alert(1)"), false);
  assert.equal(linkable("//example.com"), false);
});
