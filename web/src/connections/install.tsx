import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, dashboardContext } from "../transport";
import { useI18n } from "../ui";
import { Alert, Check, Hint, Notice } from "../components/patterns";
import { Button } from "../components/ui/button";
import type {
  ConnectionProbe,
  ConnectionToolReference,
} from "../generated/models";
import { connect } from "./connect";
import { canonicalResource, scopeIntersection } from "./scopes";
import { isConnectionOutcome, type ConnectionOutcome } from "./popup";

export function outcomeMessage(
  outcome: ConnectionOutcome,
  ja: boolean,
): string {
  const messages = {
    connected: ["Connected", "接続しました"],
    reconnected: ["Reconnected", "再接続しました"],
    denied: ["Connection was declined", "接続が許可されませんでした"],
    failed: [
      "Could not connect. Try again.",
      "接続できませんでした。もう一度お試しください。",
    ],
    closed: ["Connection window was closed", "接続ウィンドウが閉じられました"],
    redirected: ["Opening authorization…", "認証画面を開いています…"],
  };
  return messages[outcome][ja ? 1 : 0];
}

export function connectionError(reason: unknown, ja: boolean): string {
  const code = reason instanceof Error ? reason.message : String(reason);
  const messages: Record<string, [string, string]> = {
    connection_required: [
      "Connect your account to use this MCP server.",
      "この MCP サーバーを使うには、自分のアカウントで接続してください。",
    ],
    connection_scope_insufficient: [
      "Connect again and consent to the required scopes.",
      "再接続して、必要なスコープに同意してください。",
    ],
    connection_unavailable: [
      "Authorization is unavailable. Try again later.",
      "認証サービスを利用できません。しばらくしてからお試しください。",
    ],
    connection_remote_refused: [
      "Run this call on the connection owner's Node.",
      "この接続を所有する人の Node で実行してください。",
    ],
  };
  return messages[code]?.[ja ? 1 : 0] ?? code;
}

export function useConnectionProbe(
  tenant: string | null,
  resource: string | null,
) {
  // Wait for an endpoint edit to settle before making an outbound probe.
  const [ready, setReady] = useState<string | null>(null);
  useEffect(() => {
    const timer = window.setTimeout(() => setReady(resource), 350);
    return () => window.clearTimeout(timer);
  }, [resource]);
  return useQuery({
    queryKey: ["connection-probe", dashboardContext(), tenant, resource],
    queryFn: () =>
      apiFetch<ConnectionProbe>(
        `/api/tenants/${encodeURIComponent(tenant!)}/connections/probe?${new URLSearchParams({ resource: resource! })}`,
      ),
    enabled: !!tenant && !!resource && ready === resource,
    retry: false,
    staleTime: 60_000,
  });
}

/** Optional at every installation boundary; failure never disables installation. */
export function ConnectionStep({
  tenant,
  resource,
  declared,
  tool,
}: {
  tenant: string;
  resource: string;
  declared?: string[] | null;
  tool?: ConnectionToolReference;
}) {
  const canonical = canonicalResource(resource);
  if (!canonical) return null;
  // Probe and authorize the configured URL as written: a server mounted only at
  // `/mcp/` may not answer its canonical form. The server derives the identity.
  return (
    <ConnectServer
      key={JSON.stringify([tenant, canonical, declared])}
      tenant={tenant}
      resource={resource}
      declared={declared}
      tool={tool}
    />
  );
}

function ConnectServer({
  tenant,
  resource,
  declared,
  tool,
}: {
  tenant: string;
  resource: string;
  declared?: string[] | null;
  tool?: ConnectionToolReference;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const probe = useConnectionProbe(tenant, resource);
  const client = useQueryClient();
  const [selected, setSelected] = useState<string[] | null>(null);
  const [skipped, setSkipped] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const options = scopeIntersection(
    declared,
    probe.data?.scopes_supported,
    probe.data?.scope_ceiling,
  );
  const scopes = (selected ?? declared ?? []).filter((scope) =>
    options.includes(scope),
  );
  if (probe.isPending) return null;
  if (probe.isError)
    return (
      <Hint>
        {ja
          ? "接続の確認に失敗しました。インストール後にも接続できます。"
          : "Could not check authorization. You can connect after installing."}
      </Hint>
    );
  if (probe.data?.oauth !== true) return null;
  return (
    <div className="grid min-w-0 gap-2 rounded-md border border-border p-3">
      <strong className="text-sm">
        {ja ? "Connect · 個人の OAuth 接続" : "Connect · per person (OAuth)"}
      </strong>
      <span className="break-all font-mono text-xs">{resource}</span>
      <Hint>
        {ja
          ? "自分のアカウントで接続します。この手順はスキップできます。"
          : "Connect with your own account. You may skip this step."}
      </Hint>
      {!skipped &&
        options.map((scope) => (
          <Check
            key={scope}
            checked={scopes.includes(scope)}
            disabled={busy}
            onChange={(event) =>
              setSelected(
                event.target.checked
                  ? [...scopes, scope]
                  : scopes.filter((item) => item !== scope),
              )
            }
          >
            {scope}
          </Check>
        ))}
      {message && <Notice>{message}</Notice>}
      {error && <Alert>{error}</Alert>}
      <div className="flex gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError("");
            setSkipped(false);
            try {
              const outcome = await connect({ tenant, resource, scopes, tool });
              setMessage(outcomeMessage(outcome, ja));
              await client.invalidateQueries({
                queryKey: ["oauth-connections"],
              });
            } catch (reason) {
              setError(connectionError(reason, ja));
            } finally {
              setBusy(false);
            }
          }}
        >
          {ja ? "接続" : "Connect"}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          disabled={busy}
          onClick={() => {
            setSkipped(true);
            setMessage(
              ja
                ? "スキップしました。後から接続できます。"
                : "Skipped. You can connect later.",
            );
          }}
        >
          {ja ? "スキップ" : "Skip"}
        </Button>
      </div>
    </div>
  );
}

/** Registry tool transports and Agent Plugin remote servers share this step. */
export function EntryConnections({
  tenant,
  entry,
}: {
  tenant: string;
  entry: { kind: string; config: unknown };
}) {
  const config = entry.config as Record<string, unknown>;
  const transport = (
    config.transport && typeof config.transport === "object"
      ? config.transport
      : config
  ) as Record<string, unknown>;
  const servers: { resource: string; declared?: string[] }[] = [];
  if (
    entry.kind === "tool" &&
    transport.transport === "mcp" &&
    typeof transport.endpoint === "string"
  ) {
    servers.push({
      resource: transport.endpoint,
      declared: Array.isArray(transport.scopes)
        ? (transport.scopes as string[])
        : undefined,
    });
  }
  if (entry.kind === "agent" && Array.isArray(config.agent_plugins)) {
    for (const plugin of config.agent_plugins) {
      for (const server of Object.values(plugin.servers ?? {}) as {
        remote?: { url: string };
      }[]) {
        if (server.remote) servers.push({ resource: server.remote.url });
      }
    }
  }
  return (
    <>
      {servers
        .filter(
          (server, index) =>
            servers.findIndex((other) => other.resource === server.resource) ===
            index,
        )
        .map((server) => (
          <ConnectionStep key={server.resource} tenant={tenant} {...server} />
        ))}
    </>
  );
}

/** A full-page callback is reported on the page that started the connection. */
export function ConnectionOutcomeBanner({
  outcome,
}: {
  outcome: string | null;
}) {
  const { locale } = useI18n();
  const [dismissed, setDismissed] = useState(false);
  if (dismissed || !isConnectionOutcome(outcome)) return null;
  return (
    <Notice
      role="status"
      className="flex items-center justify-between px-4 py-2"
    >
      {outcomeMessage(outcome, locale === "ja-JP")}
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={() => setDismissed(true)}
      >
        {locale === "ja-JP" ? "閉じる" : "Dismiss"}
      </Button>
    </Notice>
  );
}
