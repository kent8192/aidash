/** One Decider Route: the model asked and the hosts that receive Run state. */
export type DeciderRecipient = { model: string; hosts: string[] };

/**
 * The Routes of a Decider definition in the order they are tried (ADR 0066).
 *
 * Approving a Decider version approves sending Run state to every host its
 * Routes name. A System One Route names no host: TypeSafe alone serves Jev.
 */
export function deciderRecipients(entry: {
  kind: string;
  config: unknown;
}): DeciderRecipient[] | null {
  if (entry.kind !== "decider") return null;
  if (!entry.config || typeof entry.config !== "object") return null;
  const routes = (entry.config as { routes?: unknown }).routes;
  if (!Array.isArray(routes)) return null;
  const recipients: DeciderRecipient[] = [];
  for (const route of routes) {
    if (!route || typeof route !== "object") return null;
    const { model, hosts, provider_contract } = route as {
      model?: unknown;
      hosts?: unknown;
      provider_contract?: unknown;
    };
    if (typeof model !== "string") return null;
    const named = Array.isArray(hosts)
      ? hosts.filter((host): host is string => typeof host === "string")
      : [];
    recipients.push({
      model,
      hosts:
        provider_contract === "typesafe.jev/1" && named.length === 0
          ? ["typesafe"]
          : named,
    });
  }
  return recipients.length > 0 ? recipients : null;
}
