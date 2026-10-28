import { test } from "node:test";
import assert from "node:assert/strict";
import {
  answeredRequest,
  canRequestAgain,
  defaultSelection,
  itemKey,
  needsAction,
  needsAnotherApprover,
  requestChanged,
  requestForRoot,
  restrictionFor,
  reviewSubmission,
  statusKeys,
  submittable,
} from "../src/approval-review-model.ts";

const item = (id, status, can_approve = true, extra = {}) => ({
  reference: { id, version: "1.0.0" },
  kind: "model",
  name: { en: id },
  description: { en: `${id} description` },
  digest: `sha256:${id}`,
  status,
  approved: status === "approved",
  can_approve,
  expected_revision: 2,
  restriction: null,
  proposed_restriction: { subjects: [`agent@1`], groups: [] },
  widen: false,
  installation: null,
  ...extra,
});
const review = (items, overrides = {}) => ({
  node: "default-node",
  root: { id: "agent", version: "1.0.0" },
  items,
  hidden: 0,
  closure_digest: "sha256:closure",
  ready: false,
  ...overrides,
});

test("only actionable items are selected by default", () => {
  const value = review([
    item("root", "approved"),
    item("model", "approvable"),
    item("tool", "needs_other_approver", false),
    item("copy", "needs_marketplace_activation", true, {
      installation: {
        installation: "pkg",
        revision: 1,
        digest: "digest",
        expected_activation_revision: 0,
        active_revision: null,
      },
    }),
    item("widen", "approved", true, { widen: true }),
    item("missing", "missing", false, { digest: null }),
  ]);
  assert.deepEqual(defaultSelection(value), [
    "model@1.0.0",
    "copy@1.0.0",
    "widen@1.0.0",
  ]);
  assert.equal(needsAction(value.items[4]), true);
  assert.equal(needsAction(value.items[2]), false);
});

test("a submission carries local approvals and activations in one body", () => {
  const copy = item("copy", "needs_marketplace_activation", true, {
    installation: {
      installation: "pkg",
      revision: 3,
      digest: "revision-digest",
      expected_activation_revision: 7,
      active_revision: null,
    },
  });
  const value = review([item("model", "approvable"), copy]);
  const body = reviewSubmission(
    value,
    ["model@1.0.0", "copy@1.0.0"],
    {},
    "request-id",
  );
  assert.equal(body.request, "request-id");
  assert.deepEqual(body.items, [
    {
      reference: { id: "model", version: "1.0.0" },
      digest: "sha256:model",
      expected_revision: 2,
      restriction: { subjects: ["agent@1"], groups: [] },
    },
  ]);
  assert.deepEqual(body.installations, [
    {
      installation: "pkg",
      revision: 3,
      digest: "revision-digest",
      expected_activation_revision: 7,
      restriction: { subjects: ["agent@1"], groups: [] },
    },
  ]);
});

test("an approved root can be reselected only after its restriction is edited", () => {
  const root = item("root", "approved");
  const value = review([root]);
  assert.deepEqual(defaultSelection(value), []);
  assert.deepEqual(
    reviewSubmission(value, ["root@1.0.0"], {
      "root@1.0.0": { subjects: ["alice"], groups: [] },
    }).items,
    [
      {
        reference: { id: "root", version: "1.0.0" },
        digest: "sha256:root",
        expected_revision: 2,
        restriction: { subjects: ["alice"], groups: [] },
      },
    ],
  );
});

test("an edit replaces the proposed restriction, including clearing it", () => {
  const value = item("model", "approvable");
  assert.deepEqual(restrictionFor(value, {}), value.proposed_restriction);
  assert.deepEqual(
    restrictionFor(value, {
      [itemKey(value)]: { subjects: ["alice"], groups: [] },
    }),
    { subjects: ["alice"], groups: [] },
  );
  assert.equal(restrictionFor(value, { [itemKey(value)]: null }), null);
});

test("the request for a root prefers the open one and detects a changed closure", () => {
  const requests = [
    {
      id: "old",
      state: "withdrawn",
      root: { id: "agent", version: "1.0.0" },
      closure_digest: "old",
    },
    {
      id: "open",
      state: "open",
      root: { id: "agent", version: "1.0.0" },
      closure_digest: "sha256:closure",
    },
  ];
  const open = requestForRoot(requests, { id: "agent", version: "1.0.0" });
  assert.equal(open.id, "open");
  assert.equal(requestChanged(open, review([])), false);
  assert.equal(
    requestChanged(open, review([], { closure_digest: "sha256:changed" })),
    true,
  );
});

test("every review status has a label and an explanation", () => {
  for (const status of Object.keys(statusKeys)) {
    const [label, help] = statusKeys[status];
    assert.ok(label && help);
  }
});

test("an explicit Unrestricted edit clears the restriction", () => {
  const approved = item("model", "approved", true, {
    restriction: { subjects: ["alice"], groups: [] },
  });
  const body = reviewSubmission(review([approved]), [itemKey(approved)], {
    [itemKey(approved)]: null,
  });
  assert.deepEqual(body.items[0].restriction, { subjects: [], groups: [] });
  const untouched = reviewSubmission(
    review([item("other", "approvable")]),
    ["other@1.0.0"],
    {},
  );
  assert.deepEqual(untouched.items[0].restriction, {
    subjects: ["agent@1"],
    groups: [],
  });
});

test("only an open request is answered by a submission", () => {
  const open = { id: "r1", state: "open" };
  assert.equal(answeredRequest(open, "r1"), "r1");
  for (const state of ["declined", "withdrawn", "fulfilled", "stale"])
    assert.equal(answeredRequest({ id: "r1", state }, "r1"), undefined);
  assert.equal(answeredRequest(undefined, "queued"), "queued");
  assert.equal(answeredRequest(undefined), undefined);
});

test("an edited approved item can be submitted without other work", () => {
  const approved = item("model", "approved");
  const current = review([approved], { ready: true });
  assert.equal(submittable(current, [], {}), false);
  assert.equal(
    submittable(current, [itemKey(approved)], {
      [itemKey(approved)]: { subjects: ["bob"], groups: [] },
    }),
    true,
  );
});

test("a partially authorized approver can still ask another approver", () => {
  const mixed = review([
    item("agent", "approvable"),
    item("model", "needs_other_approver", false),
  ]);
  assert.equal(needsAnotherApprover(mixed), true);
  assert.equal(
    needsAnotherApprover(review([item("agent", "approvable")])),
    false,
  );
  assert.equal(
    needsAnotherApprover(review([item("agent", "approvable")], { hidden: 1 })),
    true,
  );
  assert.equal(needsAnotherApprover(review([], { ready: true })), false);
});

test("a final request lets the requester ask again while unready", () => {
  const current = review([item("model", "needs_other_approver", false)]);
  const request = (state) => ({
    state,
    closure_digest: current.closure_digest,
  });
  assert.equal(canRequestAgain(request("open"), current), false);
  assert.equal(canRequestAgain(request("declined"), current), true);
  assert.equal(canRequestAgain(request("withdrawn"), current), true);
  assert.equal(
    canRequestAgain(request("declined"), { ...current, ready: true }),
    false,
  );
});
