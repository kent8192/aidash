import assert from "node:assert/strict";
import test from "node:test";
import { resumableRun } from "../src/collaboration/workspace-model.ts";

const paused = (semantic_reason) => ({
  phase: "READY",
  control: "PAUSED",
  recovery: { semantic_reason },
});

test("a Run whose only way on is a follow-up Task cannot be resumed (#127)", () => {
  for (const reason of [
    "invalidated",
    "context_budget",
    "durable_memory_budget",
    "embedding_model_not_found",
  ]) {
    assert.equal(resumableRun(paused(reason)), false, reason);
  }
});

test("a Run whose cause offers a retry of its unchanged Binding can be resumed", () => {
  for (const reason of [
    "pending",
    "unavailable",
    "stale",
    "retries_exhausted",
    "authority",
    "configuration",
    "allowance",
    "provider_contract",
  ]) {
    assert.equal(resumableRun(paused(reason)), true, reason);
  }
  assert.equal(resumableRun({ phase: "READY", control: "PAUSED" }), true);
});
