import assert from "node:assert/strict";
import test from "node:test";
import {
  factsForTarget,
  safetyStopSignature,
  safetyStopTarget,
} from "../src/collaboration/safety-stop-model.ts";

const input = {
  tenant: "acme",
  action: "pause",
  expected_control_revision: 3,
  reason: "runaway spend",
};

test("a retry of the same command has the same signature", () => {
  assert.equal(
    safetyStopSignature("run-1", input),
    safetyStopSignature("run-1", { ...input }),
  );
});

test("any change is a new command with its own operation ID", () => {
  const base = safetyStopSignature("run-1", input);
  for (const changed of [
    safetyStopSignature("run-2", input),
    safetyStopSignature("run-1", { ...input, tenant: "other" }),
    safetyStopSignature("run-1", { ...input, action: "cancel" }),
    safetyStopSignature("run-1", { ...input, expected_control_revision: 4 }),
    safetyStopSignature("run-1", { ...input, reason: "another reason" }),
  ])
    assert.notEqual(changed, base);
});

test("facts are dropped as soon as the Tenant or Run ID changes", () => {
  const loaded = {
    target: safetyStopTarget("acme", "run-a"),
    facts: { control_revision: 0 },
  };
  assert.deepEqual(
    factsForTarget(loaded, safetyStopTarget(" acme ", "run-a ")),
    loaded.facts,
  );
  assert.equal(
    factsForTarget(loaded, safetyStopTarget("acme", "run-b")),
    undefined,
  );
  assert.equal(
    factsForTarget(loaded, safetyStopTarget("other", "run-a")),
    undefined,
  );
  assert.equal(
    factsForTarget(undefined, safetyStopTarget("acme", "run-a")),
    undefined,
  );
});
