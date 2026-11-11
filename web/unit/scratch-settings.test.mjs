import { test } from "node:test";
import assert from "node:assert/strict";
import { bytesFromGiB, millisFromCores } from "../src/scratch-settings.ts";

test("fractional display units become integer API values", () => {
  assert.equal(bytesFromGiB("0.1"), 107374182);
  assert.equal(millisFromCores("1.0001"), 1000);
  assert.ok(Number.isInteger(bytesFromGiB("0.3")));
  assert.ok(Number.isInteger(millisFromCores("0.0007")));
});

test("focused space outside the list is loaded or reported unavailable", async () => {
  const { loadScratchSpaces } = await import("../src/scratch-settings.ts");
  const listed = { id: "recent", node: "node-a" };
  const focused = { id: "older", node: "node-a" };
  assert.deepEqual(
    await loadScratchSpaces(
      "node-a",
      "older",
      async () => [listed],
      async () => focused,
    ),
    { spaces: [listed, focused], unavailable: false },
  );
  assert.deepEqual(
    await loadScratchSpaces(
      "node-a",
      "older",
      async () => [listed],
      async () => {
        throw new Error("denied");
      },
    ),
    { spaces: [listed], unavailable: true },
  );
  assert.deepEqual(
    await loadScratchSpaces(
      "node-a",
      "older",
      async () => [listed],
      async () => ({ ...focused, node: "node-b" }),
    ),
    { spaces: [listed], unavailable: true },
  );
});
