import { test } from "node:test";
import assert from "node:assert/strict";
import { bytesFromGiB, millisFromCores } from "../src/scratch-settings.ts";

test("fractional display units become integer API values", () => {
  assert.equal(bytesFromGiB("0.1"), 107374182);
  assert.equal(millisFromCores("1.0001"), 1000);
  assert.ok(Number.isInteger(bytesFromGiB("0.3")));
  assert.ok(Number.isInteger(millisFromCores("0.0007")));
});
