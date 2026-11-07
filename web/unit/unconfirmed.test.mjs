import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  readUnconfirmed,
  unconfirmedFor,
  unconfirmedNodeName,
  unconfirmedText,
} from "../src/unconfirmed.ts";

const locale = (name) =>
  JSON.parse(
    readFileSync(new URL(`../src/locales/${name}.json`, import.meta.url)),
  );
const english = locale("en-US");
const japanese = locale("ja-JP");

const entry = (node, kind, workspace_id, count) => ({
  node: `aidash://local/${node}`,
  kind,
  workspace_id,
  count,
});

test("a list counts only its kind in the Workspaces in view", () => {
  const entries = [
    entry("b", "task", "w1", 2),
    entry("b", "artifact", "w1", 5),
    entry("b", "task", "w2", 7),
  ];
  assert.deepEqual(unconfirmedFor(entries, ["task"], ["w1"]), [
    { node: "aidash://local/b", count: 2 },
  ]);
  assert.deepEqual(unconfirmedFor(entries, ["task"], ["w1", "w2"]), [
    { node: "aidash://local/b", count: 9 },
  ]);
  assert.deepEqual(unconfirmedFor(entries, ["task", "artifact"], ["w1"]), [
    { node: "aidash://local/b", count: 7 },
  ]);
});

test("one Node adds up, and the Nodes keep an order that does not move", () => {
  const entries = [
    entry("zeta", "task", "w1", 1),
    entry("alpha", "task", "w1", 1),
    entry("zeta", "task", "w2", 2),
  ];
  const lines = unconfirmedFor(entries, ["task"], ["w1", "w2"]);
  assert.deepEqual(lines, [
    { node: "aidash://local/alpha", count: 1 },
    { node: "aidash://local/zeta", count: 3 },
  ]);
  assert.deepEqual(
    unconfirmedFor([...entries].reverse(), ["task"], ["w2", "w1"]),
    lines,
  );
});

test("nothing to say gives no line, whatever the response omitted", () => {
  assert.deepEqual(unconfirmedFor([], ["task"], ["w1"]), []);
  assert.deepEqual(unconfirmedFor(undefined, ["task"], ["w1"]), []);
  assert.deepEqual(unconfirmedFor(null, ["task"], ["w1"]), []);
  assert.deepEqual(
    unconfirmedFor([entry("a", "task", "w1", 0)], ["task"], ["w1"]),
    [],
  );
  assert.deepEqual(
    unconfirmedFor([entry("a", "task", "w1", 3)], ["task"], []),
    [],
  );
});

test("a missing or malformed field reads as empty", () => {
  assert.deepEqual(readUnconfirmed({ tasks: [] }), []);
  assert.deepEqual(readUnconfirmed(undefined), []);
  assert.deepEqual(readUnconfirmed({ unconfirmed: "x" }), []);
  const found = [entry("a", "task", "w1", 1)];
  assert.deepEqual(readUnconfirmed({ unconfirmed: found }), found);
});

test("a Node shows by its label, or by its address when the label is unknown", () => {
  const nodes = [
    { address: "aidash://local/known", label: "Lab" },
    { address: "aidash://local/unnamed", label: "" },
  ];
  assert.equal(unconfirmedNodeName(nodes, "aidash://local/known"), "Lab");
  assert.equal(
    unconfirmedNodeName(nodes, "aidash://local/unnamed"),
    "aidash://local/unnamed",
  );
  assert.equal(
    unconfirmedNodeName(nodes, "aidash://local/elsewhere"),
    "aidash://local/elsewhere",
  );
});

test("the English line names the count and the Node and promises nothing", () => {
  assert.equal(
    unconfirmedText(english.unconfirmedItems, 3, "Lab"),
    "3 items are not shown because Lab could not be reached to confirm them. They are checked again once it answers.",
  );
});

test("the Japanese line names the Node twice", () => {
  assert.equal(
    unconfirmedText(japanese.unconfirmedItems, 3, "Lab"),
    "Lab に確認できなかったため、3 件を表示していません。Lab が応答したら確認し直します。",
  );
});
