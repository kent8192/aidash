import { authenticatedFetch, transportGeneration } from "./transport";
import { stream } from "./generated/aidash";
import { SseParser, type SseFrame } from "./inference-progress";

type StreamStatus = "live" | "reconnecting";

/**
 * Read an SSE endpoint until `signal` aborts or the transport generation
 * changes, resuming from the last frame ID after each close or error.
 */
async function readEventStream({
  signal,
  cursor,
  open,
  onOpen,
  onFrame,
  onStatus,
}: {
  signal: AbortSignal;
  /** Initial `Last-Event-ID`; `undefined` sends none. */
  cursor?: string;
  open: (signal: AbortSignal, cursor?: string) => Promise<Response>;
  onOpen?: () => void;
  onFrame: (frame: SseFrame) => void;
  onStatus?: (status: StreamStatus) => void;
}) {
  const generation = transportGeneration();
  while (!signal.aborted && generation === transportGeneration()) {
    try {
      const response = await open(signal, cursor);
      if (!response.ok || !response.body) throw new Error("stream unavailable");
      if (signal.aborted || generation !== transportGeneration()) {
        await response.body.cancel();
        return;
      }
      onStatus?.("live");
      onOpen?.();
      const reader = response.body.getReader();
      try {
        const decoder = new TextDecoder();
        const parser = new SseParser();
        while (!signal.aborted && generation === transportGeneration()) {
          const { done, value } = await reader.read();
          if (signal.aborted || generation !== transportGeneration()) {
            await reader.cancel();
            return;
          }
          if (done) throw new Error("stream closed");
          for (const frame of parser.push(
            decoder.decode(value, { stream: true }),
          )) {
            if (frame.id) cursor = frame.id;
            onFrame(frame);
          }
        }
      } finally {
        await reader.cancel().catch(() => {});
        reader.releaseLock();
      }
    } catch {
      if (signal.aborted || generation !== transportGeneration()) return;
      onStatus?.("reconnecting");
      await new Promise<void>((resolve) => {
        const finish = () => {
          clearTimeout(timeout);
          signal.removeEventListener("abort", finish);
          resolve();
        };
        const timeout = setTimeout(finish, 1500);
        signal.addEventListener("abort", finish, { once: true });
      });
    }
  }
}

/** The global event stream; every ID frame invalidates dashboard queries. */
export function subscribe(
  signal: AbortSignal,
  onEvent: () => void,
  onStatus: (status: StreamStatus) => void,
) {
  return readEventStream({
    signal,
    cursor: "-1",
    open: (signal, cursor) =>
      stream(undefined, {
        signal,
        headers: { "Last-Event-ID": cursor ?? "-1" },
      }),
    onOpen: onEvent,
    onFrame: (frame) => {
      if (frame.id) onEvent();
    },
    onStatus,
  });
}

/**
 * A Run's Inference Progress stream, resuming after `cursor` (a progress
 * seq). Without a cursor the server starts at the latest attempt.
 */
export function subscribeInference(
  signal: AbortSignal,
  run: string,
  cursor: string | undefined,
  onFrame: (frame: SseFrame) => void,
) {
  return readEventStream({
    signal,
    cursor,
    open: (signal, cursor) =>
      authenticatedFetch(
        `/api/runs/${encodeURIComponent(run)}/inference/stream`,
        {
          signal,
          headers: cursor === undefined ? {} : { "Last-Event-ID": cursor },
        },
      ),
    onFrame,
  });
}
