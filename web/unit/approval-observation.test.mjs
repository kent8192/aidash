import { test } from "node:test";
import assert from "node:assert/strict";
import {
  observationProbability,
  observationRate,
  observationRange,
} from "../src/approval-observation-model.ts";

test("exact domain binary64 probabilities render without treating missing evidence as zero", () => {
  assert.equal(observationProbability("3fe0000000000000"), 0.5);
  assert.equal(observationProbability("3fc999999999999a"), 0.2);
  assert.equal(observationProbability("0000000000000000"), 0);
  for (const input of [
    "",
    "0.5",
    "7ff8000000000000",
    "bfe0000000000000",
    "4000000000000000",
  ])
    assert.equal(observationProbability(input), null);
});

test("rates preserve explicit zero denominators instead of displaying false zero agreement", () => {
  assert.equal(
    observationRate({ numerator: 0, denominator: 0, value: null }, "en-US"),
    "N/A (0 / 0)",
  );
  assert.equal(
    observationRate({ numerator: 0, denominator: 8, value: 0 }, "en-US"),
    "0% (0 / 8)",
  );
  assert.equal(
    observationRate({ numerator: 3, denominator: 4, value: 0.75 }, "en-US"),
    "75% (3 / 4)",
  );
});

test("report requests use bounded UTC half-open intervals", () => {
  assert.deepEqual(observationRange("2026-10-01", "2026-11-01"), {
    start: "2026-10-01T00:00:00.000Z",
    end: "2026-11-01T00:00:00.000Z",
  });
  for (const [start, end] of [
    ["2026-10-01", "2026-11-02"],
    ["2026-10-02", "2026-10-02"],
    ["2026-10-03", "2026-10-02"],
    ["missing", "2026-10-02"],
    ["2026-02-31", "2026-03-05"],
  ])
    assert.throws(() => observationRange(start, end), /positive UTC interval/);
});
