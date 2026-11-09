import assert from "node:assert/strict";
import test from "node:test";
import {
  declineKindKey,
  nextDeclineStep,
  offeredDecline,
} from "../src/collaboration/decline-call-model.ts";

const offered = { call: "run:1:0", tool: "task_delegate", kind: "policy" };

test("the form shows only for a paused Run the server offered a call for", () => {
  assert.deepEqual(
    offeredDecline({ declinable_call: offered }, "PAUSED"),
    offered,
  );
  assert.equal(offeredDecline({}, "PAUSED"), undefined);
  assert.equal(offeredDecline(undefined, "PAUSED"), undefined);
  // A stale response for a Run that already went on offers nothing.
  assert.equal(
    offeredDecline({ declinable_call: offered }, "ACTIVE"),
    undefined,
  );
  assert.equal(
    offeredDecline(
      { declinable_call: { ...offered, kind: "other" } },
      "PAUSED",
    ),
    undefined,
  );
});

const steps = (over) => ({
  sentence: "no",
  sent: false,
  declined: false,
  order: "sentence-first",
  ...over,
});

test("the sentence is sent once, before declining", () => {
  assert.equal(nextDeclineStep(steps({ sentence: "  " })), "decline");
  assert.equal(nextDeclineStep(steps({})), "message");
  // After the message went through, a retry only declines.
  assert.equal(nextDeclineStep(steps({ sent: true })), "decline");
  assert.equal(nextDeclineStep(steps({ sent: true, declined: true })), "done");
});

test("on another Node the call is declined first and the sentence follows", () => {
  const remote = { order: "decline-first" };
  assert.equal(nextDeclineStep(steps(remote)), "decline");
  // Declined but the message failed: a retry sends only the message, never a second decline.
  assert.equal(
    nextDeclineStep(steps({ ...remote, declined: true })),
    "message",
  );
  assert.equal(
    nextDeclineStep(steps({ ...remote, declined: true, sent: true })),
    "done",
  );
  // Without a sentence there is nothing to send afterwards.
  assert.equal(
    nextDeclineStep(steps({ ...remote, sentence: "", declined: true })),
    "done",
  );
});

test("each kind of refusal has its own words", () => {
  const keys = [
    "policy",
    "catalog_approval",
    "agent_configuration",
    "other_node",
  ].map(declineKindKey);
  assert.equal(new Set(keys).size, 4);
});
