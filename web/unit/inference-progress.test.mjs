import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SseParser,
  MAX_ATTEMPTS,
  MAX_TEXT_BYTES,
  PROGRESS_EVENT_TYPE,
  initialInferenceState,
  reduceInference,
} from "../src/inference-progress.ts";

const EVENTS = {
  started: "inference.outcome",
  outcome: "inference.outcome",
  text: "inference.delta",
  tool_call: "inference.tool_call",
};

function row(seq, attempt, kind, item) {
  return {
    id: String(seq),
    event: EVENTS[kind],
    data: JSON.stringify({
      specversion: "1.0",
      id: `run-0:${seq}`,
      source: "aidash://home",
      type: PROGRESS_EVENT_TYPE,
      subject: "run-0",
      time: "2026-10-10T00:00:00Z",
      datacontenttype: "application/json",
      data: { attempt_id: attempt, seq, kind, item },
    }),
  };
}

const started = (seq, attempt) =>
  row(seq, attempt, "started", { outcome: "pending" });
const text = (seq, attempt, value) =>
  row(seq, attempt, "text", { type: "text", text: value });

function fold(frames, state = initialInferenceState) {
  return frames.reduce(reduceInference, state);
}

test("parser joins multi-line data and strips one leading space", () => {
  const frames = new SseParser().push(
    "id: 7\nevent: inference.delta\ndata: first\ndata:  second\ndata\n\n",
  );
  assert.deepEqual(frames, [
    { id: "7", event: "inference.delta", data: "first\n second\n" },
  ]);
});

test("parser handles CRLF, lone CR and CRLF split across chunks", () => {
  const parser = new SseParser();
  assert.deepEqual(parser.push("id: 1\r\ndata: a\r"), []);
  assert.deepEqual(parser.push("\n\r\nid: 2\rdata: b\r\r"), [
    { id: "1", data: "a" },
    { id: "2", data: "b" },
  ]);
});

test("parser buffers partial frames and skips comments", () => {
  const parser = new SseParser();
  assert.deepEqual(parser.push(": keepalive\n\n"), []);
  assert.deepEqual(parser.push('event: gap\ndata: {"a"'), []);
  assert.deepEqual(parser.push(":1}\n: note\n\n"), [
    { event: "gap", data: '{"a":1}' },
  ]);
  assert.deepEqual(
    parser.push("event: error\ndata: event stream interrupted\n\n"),
    [{ event: "error", data: "event stream interrupted" }],
  );
});

test("pending text concatenates and tool calls keep the latest status", () => {
  const state = fold([
    started(1, "a"),
    text(2, "a", "Hel"),
    text(3, "a", "lo"),
    row(4, "a", "tool_call", {
      type: "tool_call",
      index: 1,
      id: "call-1",
      name: "search",
      argument_bytes: 10,
    }),
    row(5, "a", "tool_call", {
      type: "tool_call",
      index: 0,
      argument_bytes: 3,
    }),
    row(6, "a", "tool_call", {
      type: "tool_call",
      index: 1,
      argument_bytes: 42,
    }),
  ]);
  assert.equal(state.cursor, 6);
  assert.deepEqual(state.phase, { attempt: "a", phase: "started" });
  const [attempt] = state.attempts;
  assert.equal(attempt.outcome, "pending");
  assert.equal(attempt.text, "Hello");
  assert.deepEqual(attempt.toolCalls, [
    { index: 0, argumentBytes: 3 },
    { index: 1, id: "call-1", name: "search", argumentBytes: 42 },
  ]);
});

test("replayed frames after a reconnect do not duplicate text", () => {
  const frames = [started(1, "a"), text(2, "a", "one "), text(3, "a", "two")];
  const once = fold(frames);
  const replayed = fold([...frames, text(4, "a", "!")], once);
  assert.equal(replayed.attempts[0].text, "one two!");
  assert.equal(replayed.cursor, 4);
  assert.equal(fold(frames, replayed), replayed);
});

test("attempt boundaries and every outcome kind", () => {
  const reasons = [
    "cancelled",
    "lease_lost",
    "stall",
    "stream_error",
    "revoked",
  ];
  let state = fold([
    started(1, "accepted"),
    text(2, "accepted", "final"),
    row(3, "accepted", "outcome", { outcome: "accepted" }),
  ]);
  assert.equal(state.accepted, 3);
  assert.deepEqual(state.phase, { attempt: "accepted", phase: "accepted" });
  state = fold(
    [
      started(4, "discarded"),
      text(5, "discarded", "stale"),
      row(6, "discarded", "outcome", { outcome: "discarded" }),
    ],
    state,
  );
  assert.deepEqual(state.phase, {
    attempt: "discarded",
    phase: "discarded",
    reason: "correction",
  });
  assert.deepEqual(
    state.attempts.map(({ id, outcome, reason, text }) => [
      id,
      outcome,
      reason,
      text,
    ]),
    [
      ["accepted", "accepted", undefined, "final"],
      ["discarded", "discarded", "correction", "stale"],
    ],
  );
  let seq = 7;
  for (const reason of reasons) {
    state = fold(
      [
        started(seq++, reason),
        row(seq++, reason, "outcome", { outcome: "interrupted", reason }),
      ],
      state,
    );
    assert.deepEqual(state.phase, {
      attempt: reason,
      phase: "interrupted",
      reason,
    });
  }
  assert.equal(state.accepted, 3);
  assert.deepEqual(
    state.attempts.map(({ id, outcome, reason }) => [id, outcome, reason]),
    reasons.map((reason) => [reason, "interrupted", reason]),
  );
});

test("gap marks the attempt once, keeps the cursor and precedes its outcome", () => {
  const gap = {
    event: "gap",
    data: JSON.stringify({
      attempt_id: "a",
      outcome: "interrupted",
      from: 2,
      to: 9,
    }),
  };
  let state = fold([started(1, "a"), gap]);
  assert.equal(state.cursor, 1);
  assert.equal(state.attempts[0].gap, true);
  assert.equal(fold([gap], state), state);
  state = fold(
    [row(10, "a", "outcome", { outcome: "interrupted", reason: "stall" })],
    state,
  );
  assert.equal(state.attempts[0].outcome, "interrupted");
  assert.equal(state.attempts[0].reason, "stall");
  const orphan = fold([
    {
      event: "gap",
      data: JSON.stringify({ attempt_id: "b", outcome: null, from: 1, to: 4 }),
    },
  ]);
  assert.deepEqual(
    orphan.attempts.map(({ id, outcome, gap }) => [id, outcome, gap]),
    [["b", "pending", true]],
  );
});

test("unknown events, envelopes and malformed data are ignored", () => {
  const state = fold([
    { event: "error", data: "event stream interrupted" },
    { id: "1", event: "inference.delta", data: "{" },
    { id: "2", event: "inference.delta", data: JSON.stringify({ type: "x" }) },
    { event: "gap", data: "not json" },
  ]);
  assert.equal(state, initialInferenceState);
});

test("only the newest attempts are kept", () => {
  const frames = [];
  for (let index = 0; index < MAX_ATTEMPTS + 3; index++)
    frames.push(started(index + 1, `attempt-${index}`));
  const state = fold(frames);
  assert.equal(state.attempts.length, MAX_ATTEMPTS);
  assert.deepEqual(
    state.attempts.map((attempt) => attempt.id),
    Array.from({ length: MAX_ATTEMPTS }, (_, i) => `attempt-${i + 3}`),
  );
});

test("streamed text stays within the response bound", () => {
  const chunk = "é".repeat(128 * 1024);
  const frames = [started(1, "a")];
  for (let index = 0; index < 3; index++)
    frames.push(text(index + 2, "a", chunk));
  const tail = "x".repeat(256 * 1024 - 1);
  frames.push(text(5, "a", `${tail}€tail`));
  frames.push(text(6, "a", "more"));
  const state = fold(frames);
  const [attempt] = state.attempts;
  assert.equal(state.cursor, 6);
  assert.equal(attempt.truncated, true);
  assert.equal(attempt.textBytes, MAX_TEXT_BYTES - 1);
  assert.equal(
    attempt.textBytes,
    new TextEncoder().encode(attempt.text).length,
  );
  assert.equal(attempt.text, chunk.repeat(3) + tail);
});
