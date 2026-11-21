import assert from "node:assert/strict";
import test from "node:test";
import {
  budgetProblems,
  creditsToNanocredits,
  formatCredits,
  parseRecipients,
} from "../src/budgets-model.ts";

test("credits become nanocredits without floating point", () => {
  assert.equal(creditsToNanocredits("12.5"), 12_500_000_000);
  assert.equal(creditsToNanocredits("0.000000001"), 1);
  assert.equal(creditsToNanocredits(""), undefined);
  assert.equal(creditsToNanocredits("-1"), null);
  assert.equal(creditsToNanocredits("0"), null);
  assert.equal(creditsToNanocredits("1.0000000001"), null);
  assert.equal(formatCredits(12_500_000_000), "12.50");
});

test("recipients split on lines and commas", () => {
  assert.deepEqual(parseRecipients("a@example.com, b@example.com\n\n c@x.io"), [
    "a@example.com",
    "b@example.com",
    "c@x.io",
  ]);
});

test("a Budget needs a limit, soft below hard and a recipient for mail", () => {
  assert.deepEqual(budgetProblems({ soft: "", hard: "", recipients: "" }), [
    "no_limit",
  ]);
  assert.deepEqual(
    budgetProblems({ soft: "10", hard: "10", recipients: "a@example.com" }),
    ["soft_not_below_hard"],
  );
  assert.deepEqual(budgetProblems({ soft: "5", hard: "", recipients: "" }), [
    "no_recipient",
  ]);
  assert.deepEqual(
    budgetProblems({ soft: "5", hard: "10", recipients: "not an address" }),
    ["bad_address"],
  );
  assert.deepEqual(
    budgetProblems({ soft: "", hard: "10", recipients: "" }),
    [],
  );
});
