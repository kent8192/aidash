import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { bundleForTest } from "./bundle-for-test.mjs";

const { bytesFromGiB, millisFromCores, loadScratchSpaces, ApiError } =
  await bundleForTest(import.meta.url, {
    stdin: {
      contents: `
        export * from "../src/scratch-settings.ts";
        export { ApiError } from "../src/transport.ts";
      `,
      resolveDir: fileURLToPath(new URL(".", import.meta.url)),
    },
  });

test("fractional display units become integer API values", () => {
  assert.equal(bytesFromGiB("0.1"), 107374182);
  assert.equal(millisFromCores("1.0001"), 1000);
  assert.ok(Number.isInteger(bytesFromGiB("0.3")));
  assert.ok(Number.isInteger(millisFromCores("0.0007")));
});

test("focused space outside the list is loaded or reported unavailable", async () => {
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
  for (const status of [403, 404]) {
    assert.deepEqual(
      await loadScratchSpaces(
        "node-a",
        "older",
        async () => [listed],
        async () => {
          throw new ApiError("Cannot read space", status);
        },
      ),
      { spaces: [listed], unavailable: true },
    );
  }
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

test("a server or network failure reading the focused space rejects the load", async () => {
  for (const error of [
    new ApiError("Server error", 500),
    new TypeError("Failed to fetch"),
  ]) {
    await assert.rejects(
      loadScratchSpaces(
        "node-a",
        "older",
        async () => [{ id: "recent", node: "node-a" }],
        async () => {
          throw error;
        },
      ),
      (actual) => actual === error,
    );
  }
});
