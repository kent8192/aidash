import { transportGeneration } from "./transport";
import { stream } from "./generated/aidash";
const MAX_SSE_FRAME_CHARS = 64 * 1024;
const TEXT_CHUNK_BYTES = 16 * 1024;
export async function subscribe(
  signal: AbortSignal,
  onEvent: () => void,
  onStatus: (status: "live" | "reconnecting") => void,
) {
  let cursor = "-1";
  const generation = transportGeneration();
  while (!signal.aborted && generation === transportGeneration()) {
    try {
      const response = await stream(undefined, {
        signal,
        headers: {
          "Last-Event-ID": cursor,
        },
      });
      if (!response.ok || !response.body) throw new Error("stream unavailable");
      if (signal.aborted || generation !== transportGeneration()) {
        await response.body.cancel();
        return;
      }
      onStatus("live");
      onEvent();
      const reader = response.body.getReader();
      try {
        const decoder = new TextDecoder();
        let buffer = "";
        while (!signal.aborted && generation === transportGeneration()) {
          const { done, value } = await reader.read();
          if (signal.aborted || generation !== transportGeneration()) {
            await reader.cancel();
            return;
          }
          if (done) throw new Error("stream closed");
          for (
            let offset = 0;
            offset < value.length;
            offset += TEXT_CHUNK_BYTES
          ) {
            buffer += decoder.decode(
              value.subarray(offset, offset + TEXT_CHUNK_BYTES),
              {
                stream: true,
              },
            );
            buffer = buffer.replace(/\r\n/g, "\n");
            let end;
            while ((end = buffer.indexOf("\n\n")) >= 0) {
              if (end > MAX_SSE_FRAME_CHARS)
                throw new Error("SSE frame too large");
              const block = buffer.slice(0, end);
              buffer = buffer.slice(end + 2);
              const id = block
                .split("\n")
                .find((line) => line.startsWith("id:"))
                ?.slice(3)
                .trim();
              if (id) {
                cursor = id;
                onEvent();
              }
            }
            if (buffer.length > MAX_SSE_FRAME_CHARS)
              throw new Error("SSE frame too large");
          }
        }
      } finally {
        await reader.cancel().catch(() => {});
        reader.releaseLock();
      }
    } catch {
      if (signal.aborted || generation !== transportGeneration()) return;
      onStatus("reconnecting");
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
