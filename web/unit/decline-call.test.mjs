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

test("the sentence is sent once, before declining", () => {
  assert.equal(nextDeclineStep({ sentence: "  ", sent: false }), "decline");
  assert.equal(nextDeclineStep({ sentence: "no", sent: false }), "message");
  // After the message went through, a retry only declines.
  assert.equal(nextDeclineStep({ sentence: "no", sent: true }), "decline");
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
