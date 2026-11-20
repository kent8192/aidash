import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  MAX_STANDING_INSTRUCTIONS_BYTES,
  STANDING_INSTRUCTIONS_PROBLEM_MESSAGES,
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

test("each problem has its own message in both locales", () => {
  const messages = Object.values(STANDING_INSTRUCTIONS_PROBLEM_MESSAGES);
  assert.equal(new Set(messages).size, messages.length);
  assert.equal(
    STANDING_INSTRUCTIONS_PROBLEM_MESSAGES[
      standingInstructionsProblems("a\0b")[0]
    ],
    "instructionsNul",
  );
  assert.equal(
    STANDING_INSTRUCTIONS_PROBLEM_MESSAGES[
      standingInstructionsProblems(
        "a".repeat(MAX_STANDING_INSTRUCTIONS_BYTES + 1),
      )[0]
    ],
    "instructionsTooLong",
  );
  for (const name of ["en-US", "ja-JP"]) {
    const locale = JSON.parse(
      readFileSync(new URL(`../src/locales/${name}.json`, import.meta.url)),
    );
    for (const key of messages) assert.ok(locale[key], `${name}: ${key}`);
  }
});
