import { useRef, useState } from "react";
import {
  SECRET_HEADER,
  describeSecret,
  editSecret,
  isDeclaredReadOnly,
  readListing,
  serverSettings,
  toggleReadOnly,
  unlistedReadOnly,
  withServerSettings,
  type AgentPluginRef,
  type AttachListing,
} from "./agent-plugins";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Alert, Check, Hint } from "./components/patterns";
import { apiFetch } from "./transport";
import { Field, useI18n } from "./ui";

type Config = Record<string, unknown>;

const reasonLabels: Record<string, string> = {
  connection_required: "agentPluginListConnect",
  connection_scope_insufficient: "agentPluginListConnect",
  secret_unavailable: "agentPluginListSecret",
  secret_rejected: "agentPluginListRejected",
};

/**
 * How one remote server of an attached Agent Plugin authenticates, and which
 * of its tools are declared Read-only (ADR 0100, ADR 0073 amended
 * 2026-11-16). The person's OAuth Connection comes first; the pasted secret
 * named here is used when the server offers no OAuth. Read-only can be
 * declared only over a listing made with the person's own authentication;
 * Run start checks every declared digest again.
 */
export function AgentPluginServerSettings({
  plugin,
  server,
  config,
  change,
}: {
  plugin: AgentPluginRef;
  server: string;
  config: Config;
  change: (config: Config) => void;
}) {
  const { t } = useI18n();
  const settings = serverSettings(config, plugin.name, server);
  const secret = settings.secret;
  const declared = settings.read_only ?? [];
  const [listing, setListing] = useState<AttachListing | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const request = useRef(0);
  const update = (next: typeof settings) =>
    change(withServerSettings(config, plugin.name, server, next));
  const setSecret = (patch: Partial<NonNullable<typeof secret>>) => {
    update({ ...settings, secret: editSecret(secret, patch) });
    // A listing made with the previous secret no longer applies, and its
    // answer is ignored, so it must not leave the form busy.
    request.current += 1;
    setListing(null);
    setBusy(false);
  };

  const list = async () => {
    const current = ++request.current;
    setBusy(true);
    setError("");
    setListing(null);
    try {
      const result = readListing(
        await apiFetch<unknown>("/api/agent-plugins/tools", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            digest: plugin.digest,
            server,
            ...(secret?.name ? { secret } : {}),
          }),
        }),
      );
      if (current !== request.current) return;
      if (!result) throw new Error(t("agentPluginListError"));
      setListing(result);
    } catch (cause) {
      if (current !== request.current) return;
      setError(
        cause instanceof Error && cause.message
          ? cause.message
          : t("agentPluginListError"),
      );
    } finally {
      if (current === request.current) setBusy(false);
    }
  };

  const described = secret?.name ? describeSecret(secret) : null;
  const unlisted = unlistedReadOnly(declared, listing);
  return (
    <div className="agent-plugin-server-settings grid min-w-0 gap-2 border-l-2 border-border pl-3">
      <span className="font-mono text-xs font-medium text-foreground [overflow-wrap:anywhere]">
        {server}
      </span>
      <Hint>{t("agentPluginSecretHelp")}</Hint>
      <div className="grid min-w-0 gap-2 sm:grid-cols-2">
        <Field label="Tenant Secret">
          <Input
            value={secret?.name ?? ""}
            placeholder="api-key"
            pattern="[a-z0-9][a-z0-9_-]{0,63}"
            onChange={(event) => setSecret({ name: event.target.value })}
          />
        </Field>
        <Field label={t("agentPluginSecretHeader")}>
          <Input
            value={secret?.header ?? ""}
            placeholder={SECRET_HEADER}
            disabled={!secret?.name}
            onChange={(event) => setSecret({ header: event.target.value })}
          />
        </Field>
      </div>
      {described?.header.toLowerCase() === SECRET_HEADER.toLowerCase() && (
        <Check
          checked={secret?.form === "raw"}
          onChange={(event) =>
            setSecret({ form: event.target.checked ? "raw" : undefined })
          }
        >
          {t("agentPluginSecretRawAuthorization")}
        </Check>
      )}
      <Check
        checked={secret?.owner === "personal"}
        disabled={!secret?.name}
        onChange={(event) =>
          setSecret({ owner: event.target.checked ? "personal" : "any" })
        }
      >
        {t("toolPersonalSecret")}
      </Check>
      {described && (
        <Hint>
          {described.header} (
          {t(
            described.form === "bearer"
              ? "agentPluginSecretBearer"
              : "agentPluginSecretRaw",
          )}
          )
        </Hint>
      )}
      <Button
        variant="outline"
        size="sm"
        type="button"
        className="justify-self-start"
        disabled={busy}
        onClick={() => void list()}
      >
        {busy ? t("loading") : t("agentPluginListTools")}
      </Button>
      {error && <Alert>{error}</Alert>}
      {listing?.outcome === "unavailable" && (
        <Alert>
          {t(reasonLabels[listing.reason] ?? "agentPluginListUnavailable")} (
          <span className="font-mono">{listing.reason}</span>)
        </Alert>
      )}
      {listing?.outcome === "listed" &&
        (listing.tools.length === 0 ? (
          <Hint>{t("agentPluginListEmpty")}</Hint>
        ) : (
          <fieldset className="grid min-w-0 gap-1">
            <legend className="text-xs font-semibold text-foreground">
              {t("agentPluginReadOnly")}
            </legend>
            <Hint>{t("agentPluginReadOnlyHelp")}</Hint>
            {listing.tools.map((tool) => (
              <Check
                key={tool.name}
                checked={isDeclaredReadOnly(declared, tool)}
                onChange={(event) =>
                  update({
                    ...settings,
                    read_only: toggleReadOnly(
                      declared,
                      tool,
                      event.target.checked,
                    ),
                  })
                }
              >
                <span className="font-mono">{tool.name}</span>
              </Check>
            ))}
          </fieldset>
        ))}
      {unlisted.length > 0 && (
        <fieldset className="grid min-w-0 gap-1">
          <legend className="text-xs font-semibold text-foreground">
            {t("agentPluginReadOnly")}
          </legend>
          <Hint>
            {t(
              listing?.outcome === "listed"
                ? "agentPluginReadOnlyUnlisted"
                : "agentPluginReadOnlyDeclared",
            )}
          </Hint>
          {unlisted.map((tool) => (
            <Check
              key={tool.name}
              checked
              onChange={(event) => {
                if (event.target.checked) return;
                update({
                  ...settings,
                  read_only: toggleReadOnly(declared, tool, false),
                });
              }}
            >
              <span className="font-mono">{tool.name}</span>
            </Check>
          ))}
        </fieldset>
      )}
    </div>
  );
}
