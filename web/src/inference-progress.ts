// Dependency-free SSE frame parsing and the Inference Progress reducer, so the
// node unit tests can import them without the generated client or locales.

export type SseFrame = { id?: string; event?: string; data: string };

/** Incremental `text/event-stream` parser. Comment-only blocks yield nothing. */
export class SseParser {
  private buffer = "";
  /** The previous chunk ended in CR, so a leading LF completes that CRLF. */
  private pendingLf = false;

  push(chunk: string): SseFrame[] {
    if (!chunk) return [];
    if (this.pendingLf && chunk.startsWith("\n")) chunk = chunk.slice(1);
    this.pendingLf = chunk.endsWith("\r");
    const text = (this.buffer + chunk).replace(/\r\n?/g, "\n");
    const frames: SseFrame[] = [];
    let start = 0;
    let end;
    while ((end = text.indexOf("\n\n", start)) >= 0) {
      const frame = parseBlock(text.slice(start, end));
      if (frame) frames.push(frame);
      start = end + 2;
    }
    this.buffer = text.slice(start);
    return frames;
  }
}

function parseBlock(block: string): SseFrame | undefined {
  let id: string | undefined;
  let event: string | undefined;
  const data: string[] = [];
  let fields = false;
  for (const line of block.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const colon = line.indexOf(":");
    const name = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (name === "data") data.push(value);
    else if (name === "event") event = value;
    else if (name === "id" && !value.includes("\0")) id = value;
    else continue;
    fields = true;
  }
  if (!fields) return undefined;
  const frame: SseFrame = { data: data.join("\n") };
  if (id !== undefined) frame.id = id;
  if (event !== undefined) frame.event = event;
  return frame;
}

export const PROGRESS_EVENT_TYPE = "aidash.inference.progress.v1";
/** Attempts kept on the client; older ones are dropped. */
export const MAX_ATTEMPTS = 5;
/** Streamed text kept per attempt, matching the server's 1 MiB response bound. */
export const MAX_TEXT_BYTES = 1024 * 1024;

export type InferenceOutcome =
  | "pending"
  | "accepted"
  | "discarded"
  | "interrupted";
export type InferencePhase = "started" | Exclude<InferenceOutcome, "pending">;

export type ToolCallProgress = {
  index: number;
  id?: string;
  name?: string;
  argumentBytes: number;
};

export type InferenceAttempt = {
  id: string;
  outcome: InferenceOutcome;
  /** Interruption reason, or `correction` for a discarded attempt. */
  reason?: string;
  text: string;
  textBytes: number;
  truncated: boolean;
  toolCalls: ToolCallProgress[];
  /** Retention removed some of this attempt's progress. */
  gap: boolean;
};

export type InferenceState = {
  /** Last delivered `progress_seq`; frames at or before it are replays. */
  cursor: number | null;
  attempts: InferenceAttempt[];
  /** Latest phase change, for a polite announcement. */
  phase: { attempt: string; phase: InferencePhase; reason?: string } | null;
  /** Seq of the latest accepted outcome row. */
  accepted: number | null;
};

export const initialInferenceState: InferenceState = {
  cursor: null,
  attempts: [],
  phase: null,
  accepted: null,
};

function utf8Length(text: string): number {
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code < 0xdc00 && i + 1 < text.length) {
      bytes += 4;
      i++;
    } else bytes += 3;
  }
  return bytes;
}

/** The longest prefix of `text` within `budget` UTF-8 bytes. */
function utf8Prefix(text: string, budget: number): string {
  let bytes = 0;
  let i = 0;
  while (i < text.length) {
    const code = text.charCodeAt(i);
    const pair = code >= 0xd800 && code < 0xdc00 && i + 1 < text.length;
    const size = code < 0x80 ? 1 : code < 0x800 ? 2 : pair ? 4 : 3;
    if (bytes + size > budget) break;
    bytes += size;
    i += pair ? 2 : 1;
  }
  return text.slice(0, i);
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function parse(data: string): Record<string, unknown> | undefined {
  try {
    return record(JSON.parse(data));
  } catch {
    return undefined;
  }
}

const OUTCOMES: Record<string, true> = {
  pending: true,
  accepted: true,
  discarded: true,
  interrupted: true,
};

/** Replace or append an attempt, keeping only the newest `MAX_ATTEMPTS`. */
function put(
  attempts: InferenceAttempt[],
  attempt: InferenceAttempt,
): InferenceAttempt[] {
  const index = attempts.findIndex((value) => value.id === attempt.id);
  const next =
    index < 0
      ? [...attempts, attempt]
      : attempts.map((value, at) => (at === index ? attempt : value));
  return next.length > MAX_ATTEMPTS ? next.slice(-MAX_ATTEMPTS) : next;
}

/** The known attempt, or a new pending one when its `started` row is gone. */
function attemptFor(state: InferenceState, id: string): InferenceAttempt {
  const known = state.attempts.find((value) => value.id === id);
  if (known) return known;
  return {
    id,
    outcome: "pending",
    text: "",
    textBytes: 0,
    truncated: false,
    toolCalls: [],
    gap: false,
  };
}

/** Fold one run-stream frame into the attempts state. */
export function reduceInference(
  state: InferenceState,
  frame: SseFrame,
): InferenceState {
  if (frame.event === "gap") {
    const gap = parse(frame.data);
    const id = gap?.attempt_id;
    if (typeof id !== "string") return state;
    const attempt = attemptFor(state, id);
    if (attempt.gap) return state;
    const outcome =
      typeof gap?.outcome === "string" && OUTCOMES[gap.outcome]
        ? (gap.outcome as InferenceOutcome)
        : undefined;
    return {
      ...state,
      attempts: put(state.attempts, {
        ...attempt,
        gap: true,
        outcome:
          attempt.outcome === "pending" && outcome ? outcome : attempt.outcome,
      }),
    };
  }
  if (!frame.event?.startsWith("inference.")) return state;
  const envelope = parse(frame.data);
  if (envelope?.type !== PROGRESS_EVENT_TYPE) return state;
  const data = record(envelope.data);
  const seq = data?.seq;
  const id = data?.attempt_id;
  const item = record(data?.item);
  if (typeof seq !== "number" || typeof id !== "string" || !item) return state;
  if (state.cursor !== null && seq <= state.cursor) return state;
  const next: InferenceState = { ...state, cursor: seq };
  const attempt = attemptFor(state, id);
  switch (data?.kind) {
    case "started":
      next.attempts = put(state.attempts, attempt);
      next.phase = { attempt: id, phase: "started" };
      return next;
    case "text": {
      if (typeof item.text !== "string" || attempt.truncated) {
        next.attempts = put(state.attempts, attempt);
        return next;
      }
      const size = utf8Length(item.text);
      const room = MAX_TEXT_BYTES - attempt.textBytes;
      const text = size <= room ? item.text : utf8Prefix(item.text, room);
      next.attempts = put(state.attempts, {
        ...attempt,
        text: attempt.text + text,
        textBytes: attempt.textBytes + (size <= room ? size : utf8Length(text)),
        truncated: size > room,
      });
      return next;
    }
    case "tool_call": {
      if (typeof item.index !== "number") return next;
      const previous = attempt.toolCalls.find(
        (call) => call.index === item.index,
      );
      const call: ToolCallProgress = {
        index: item.index,
        argumentBytes: Math.max(
          previous?.argumentBytes ?? 0,
          typeof item.argument_bytes === "number" ? item.argument_bytes : 0,
        ),
      };
      const callId = typeof item.id === "string" ? item.id : previous?.id;
      const name = typeof item.name === "string" ? item.name : previous?.name;
      if (callId !== undefined) call.id = callId;
      if (name !== undefined) call.name = name;
      next.attempts = put(state.attempts, {
        ...attempt,
        toolCalls: previous
          ? attempt.toolCalls.map((value) =>
              value.index === call.index ? call : value,
            )
          : [...attempt.toolCalls, call].sort((a, b) => a.index - b.index),
      });
      return next;
    }
    case "outcome": {
      const outcome = item.outcome;
      if (
        outcome !== "accepted" &&
        outcome !== "discarded" &&
        outcome !== "interrupted"
      )
        return next;
      const reason =
        outcome === "discarded"
          ? "correction"
          : outcome === "interrupted" && typeof item.reason === "string"
            ? item.reason
            : undefined;
      const ended: InferenceAttempt = { ...attempt, outcome };
      if (reason) ended.reason = reason;
      else delete ended.reason;
      next.attempts = put(state.attempts, ended);
      next.phase = reason
        ? { attempt: id, phase: outcome, reason }
        : { attempt: id, phase: outcome };
      if (outcome === "accepted") next.accepted = seq;
      return next;
    }
    default:
      return next;
  }
}
