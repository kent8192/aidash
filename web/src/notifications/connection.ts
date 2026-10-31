/** Connect actions use the full authorized request payload, never truncated notice text. */
export type ConnectionAction = {
  tenant: string;
  resource: string;
  scopes: string[];
};
export function connectionAction(
  request: { kind: string; prompt: string; response: unknown } | undefined,
  tenant: string | undefined,
): ConnectionAction | null {
  if (
    !tenant ||
    request?.kind !== "MCP_CONNECTION" ||
    request.response !== null
  )
    return null;
  try {
    const payload: unknown = JSON.parse(request.prompt);
    if (!payload || typeof payload !== "object") return null;
    const { resource, scopes } = payload as {
      resource?: unknown;
      scopes?: unknown;
    };
    if (typeof resource !== "string" || !resource) return null;
    if (
      scopes !== undefined &&
      (!Array.isArray(scopes) ||
        !scopes.every((scope) => typeof scope === "string"))
    )
      return null;
    return { tenant, resource, scopes: (scopes as string[] | undefined) ?? [] };
  } catch {
    return null;
  }
}
