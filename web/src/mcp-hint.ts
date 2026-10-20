/** The transport, endpoint and tool an MCP hint was asked about. */
export type HintQuery = {
  transport: string;
  endpoint: string;
  toolName: string;
};

/** Whether a hint answer still belongs to the tool currently in the form. */
export function sameQuery(
  query: HintQuery,
  transport: string,
  endpoint: string,
  toolName: string,
): boolean {
  return (
    query.transport === transport &&
    query.endpoint === endpoint &&
    query.toolName === toolName
  );
}

/**
 * The replay choice after a hint answer: a read-only hint prefills `read_only`;
 * any other answer undoes only a prefill this code made, never a person's own
 * choice. `prefilled` says whether the current `read_only` came from a hint.
 */
export function replayAfterHint(
  replay: string,
  readOnly: boolean | null | undefined,
  prefilled: boolean,
): { replay: string; prefilled: boolean } {
  if (readOnly === true) return { replay: "read_only", prefilled: true };
  if (prefilled) return { replay: "unsafe", prefilled: false };
  return { replay, prefilled };
}

/**
 * The replay choice after the endpoint or tool name changes: an automatic
 * read-only prefill belonged to the old tool and is undone.
 */
export function replayAfterQueryChange(
  replay: string,
  prefilled: boolean,
): string {
  return prefilled ? "unsafe" : replay;
}
