import assert from "node:assert/strict";
import test from "node:test";
import {
  MAX_STANDING_INSTRUCTIONS_BYTES,
  standingInstructionsProblems,
  utf8Bytes,
} from "../src/standing-instructions-model.ts";

test("the largest text is accepted and one byte more is not", () => {
  assert.deepEqual(
    standingInstructionsProblems("a".repeat(MAX_STANDING_INSTRUCTIONS_BYTES)),
    [],
  );
  assert.deepEqual(
    standingInstructionsProblems(
      "a".repeat(MAX_STANDING_INSTRUCTIONS_BYTES + 1),
    ),
    ["too_long"],
  );
});

test("bytes are counted in UTF-8, as the server counts them", () => {
  assert.equal(utf8Bytes("日本語"), 9);
  const japanese = "あ".repeat(
    Math.floor(MAX_STANDING_INSTRUCTIONS_BYTES / 3) + 1,
  );
  assert.deepEqual(standingInstructionsProblems(japanese), ["too_long"]);
});

test("NUL is refused", () => {
  assert.deepEqual(standingInstructionsProblems("a\0b"), ["nul"]);
});
