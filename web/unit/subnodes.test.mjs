import assert from "node:assert/strict";
import test from "node:test";
import {
  subnodeActive,
  subnodeLimitsProblems,
  subnodeTitle,
} from "../src/subnodes-model.ts";

const limits = {
  members_per_subnode: 5,
  subnodes_per_run: 3,
  remote_runs_may_create: false,
};

test("the default limits are accepted", () => {
  assert.deepEqual(subnodeLimitsProblems(limits), []);
});

test("the range ends are accepted and the values past them are not", () => {
  assert.deepEqual(
    subnodeLimitsProblems({
      ...limits,
      members_per_subnode: 20,
      subnodes_per_run: 10,
    }),
    [],
  );
  assert.deepEqual(
    subnodeLimitsProblems({
      ...limits,
      members_per_subnode: 21,
      subnodes_per_run: 0,
    }),
    ["members_per_subnode", "subnodes_per_run"],
  );
});

test("empty and fractional values are refused", () => {
  assert.deepEqual(
    subnodeLimitsProblems({ ...limits, members_per_subnode: Number("") + NaN }),
    ["members_per_subnode"],
  );
  assert.deepEqual(
    subnodeLimitsProblems({ ...limits, subnodes_per_run: 2.5 }),
    ["subnodes_per_run"],
  );
});

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
