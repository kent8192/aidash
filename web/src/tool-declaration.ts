export type ToolReplay = "read_only" | "idempotent" | "unsafe";

/**
 * What a Tenant's HTTP or MCP tool version declares about its one `invoke`
 * operation, or `null` for any other definition (ADR 0078).
 *
 * Only `read_only` is a Read-only operation, whose calls need no Tool call
 * approval. The server reads the same field, and any other value asks.
 */
export function toolReplay(entry: {
  kind: string;
  config: unknown;
}): ToolReplay | null {
  if (entry.kind !== "tool") return null;
  if (!entry.config || typeof entry.config !== "object") return null;
  const transport = (entry.config as { transport?: unknown }).transport;
  if (!transport || typeof transport !== "object") return null;
  const { transport: kind, replay } = transport as {
    transport?: unknown;
    replay?: unknown;
  };
  if (kind !== "http" && kind !== "mcp") return null;
  return replay === "read_only" || replay === "idempotent" ? replay : "unsafe";
}
