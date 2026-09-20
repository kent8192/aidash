export async function api<T>(
  path: string,
  body?: unknown,
  method = body === undefined ? "GET" : "POST",
): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${sessionStorage.getItem("aidash-token") ?? ""}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) {
    const data = await response
      .json()
      .catch(() => ({ error: response.statusText }));
    throw new Error(data.error ?? response.statusText);
  }
  return response.json() as Promise<T>;
}
export async function subscribe(
  signal: AbortSignal,
  onEvent: () => void,
  onStatus: (status: "live" | "reconnecting") => void,
) {
  let cursor = "0";
  while (!signal.aborted) {
    try {
      const response = await fetch("/api/events/stream", {
        signal,
        headers: {
          Authorization: `Bearer ${sessionStorage.getItem("aidash-token") ?? ""}`,
          "Last-Event-ID": cursor,
        },
      });
      if (!response.ok || !response.body) throw new Error("stream unavailable");
      onStatus("live");
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      while (!signal.aborted) {
        const { done, value } = await reader.read();
        if (done) throw new Error("stream closed");
        buffer += decoder
          .decode(value, { stream: true })
          .replace(/\r\n/g, "\n");
        let end;
        while ((end = buffer.indexOf("\n\n")) >= 0) {
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
      }
    } catch {
      if (signal.aborted) return;
      onStatus("reconnecting");
      await new Promise<void>((resolve) => {
        const timeout = setTimeout(resolve, 1500);
        signal.addEventListener(
          "abort",
          () => {
            clearTimeout(timeout);
            resolve();
          },
          { once: true },
        );
      });
    }
  }
}
