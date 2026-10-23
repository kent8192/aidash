/**
 * The origin a Keyless definition sends requests to, or `null` (ADR 0036).
 *
 * A model or embedding definition that names neither `provider_credential` nor
 * `plan_connection` calls
 * its endpoint without any grant, and Aidash promises nothing about what that
 * endpoint retains. This mirrors the server's rule: only these two kinds call a
 * model provider, and naming a Provider Catalog entry makes a call keyed.
 */
export function keylessOrigin(entry: {
  kind: string;
  config: unknown;
}): string | null {
  if (entry.kind !== "model" && entry.kind !== "embedding") return null;
  if (!entry.config || typeof entry.config !== "object") return null;
  const config = entry.config as {
    provider_credential?: unknown;
    plan_connection?: unknown;
    endpoint?: unknown;
  };
  if (config.provider_credential != null || config.plan_connection != null)
    return null;
  if (typeof config.endpoint !== "string") return null;
  try {
    const url = new URL(config.endpoint);
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.origin
      : null;
  } catch {
    return null;
  }
}
