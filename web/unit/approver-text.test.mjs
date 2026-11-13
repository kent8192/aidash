import { test } from "node:test";
import assert from "node:assert/strict";
import {
  approverText,
  decisionErrorText,
} from "../src/capabilities/approver-text.ts";

test("a waiting approval names no approver and no count", () => {
  const card = { kind: "disclosure", state: "pending", decided_by: null };
  assert.equal(approverText(card, false), "Waiting for an eligible approver");
  assert.equal(approverText(card, true), "承認できる人の判断を待っています");
});

test("a decided approval names who allowed or denied it", () => {
  assert.equal(
    approverText(
      { kind: "tool_call", state: "approved", decided_by: "carol" },
      false,
    ),
    "carol allowed it",
  );
  assert.equal(
    approverText(
      { kind: "tool_call", state: "denied", decided_by: "carol" },
      true,
    ),
    "carol が拒否しました",
  );
  assert.equal(
    approverText({ kind: "grant", state: "revoked", decided_by: "bob" }, false),
    "bob allowed it",
  );
});

test("an approval nobody decided names nobody once it stops waiting", () => {
  assert.equal(
    approverText(
      { kind: "disclosure", state: "expired", decided_by: null },
      false,
    ),
    null,
  );
});

test("nobody to ask keeps its own text", () => {
  assert.equal(
    approverText({ kind: "outbound", state: "blocked" }, false),
    "Blocked: no eligible approver",
  );
});

test("a late decision says who decided first, apart from expiry", () => {
  const late = (outcome) => ({
    error: {
      code: "APPROVAL_ALREADY_DECIDED",
      message: "APPROVAL_ALREADY_DECIDED",
      retryable: false,
      details: { decided_by: "carol", outcome },
    },
  });
  assert.equal(
    decisionErrorText(late("denied"), false),
    "carol already denied this approval.",
  );
  assert.equal(
    decisionErrorText(late("allowed"), true),
    "carol がすでに許可しました。",
  );
  assert.match(
    decisionErrorText({ error: { code: "APPROVAL_STALE_OR_EXPIRED" } }, false),
    /expired or changed/,
  );
  assert.equal(
    decisionErrorText({ error: { code: "FORBIDDEN" } }, false),
    null,
  );
  assert.equal(decisionErrorText(undefined, false), null);
});
