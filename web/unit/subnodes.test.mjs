import assert from "node:assert/strict";
import test from "node:test";
import { subnodeActive, subnodeTitle } from "../src/subnodes-model.ts";

test("an untitled Subnode is named by the head of its id", () => {
  assert.equal(
    subnodeTitle({ id: "0190aaaa-bbbb-cccc-dddd-eeeeeeeeeeee", title: null }),
    "0190aaaa",
  );
  assert.equal(subnodeTitle({ id: "x", title: "  " }), "x");
  assert.equal(subnodeTitle({ id: "x", title: "Review team" }), "Review team");
});

test("only an active Subnode can be dissolved", () => {
  assert.equal(subnodeActive({ state: "active" }), true);
  assert.equal(subnodeActive({ state: "dissolved" }), false);
});
