import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Badge as StatusBadge } from "./components/ui/badge";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { Alert, Notice } from "./components/patterns";
import { Empty, Field, Panel, useI18n, type StatusTone } from "./ui";
import { ApiError } from "./transport";
import type { State } from "./types";
import {
  providerCredentialList,
  providerCredentialGet,
  providerCredentialRevoke,
  providerCredentialDelete,
  providerCredentialBindingList,
  providerCredentialBindingUpdate,
  providerAuthorizationStart,
} from "./generated/aidash";

const outcomes = [
  "connected",
  "unbound",
  "reconnected",
  "denied",
  "failed",
] as const;
type Outcome = (typeof outcomes)[number];

/** OpenRouter's owner-only key-settings page; nothing else is linked. */
const keySettingsPattern = /^https:\/\/openrouter\.ai\/keys\/[0-9a-f]{64}$/;

/** The Tenant whose Provider Authorization the browser left for. */
const tenantKey = "aidash-provider-authorization-tenant";

/**
 * The Tenant a Deployment Administrator returns to from Provider Authorization: the callback
 * reloads the dashboard, and the Deployment Administrator's Tenant is otherwise page state.
 */
function returningTenant(): string {
  return sessionStorage.getItem("aidash-provider-authorization")
    ? (sessionStorage.getItem(tenantKey) ?? "")
    : "";
}

/**
 * Reads the callback's `provider_authorization` outcome; the
 * settings page of a key OpenRouter still holds but Aidash does not store (one
 * issued for a failed write, or the key a reconnect replaced); and whether
 * validation warned about the connected key. The outcome belongs only to the
 * Tenant the authorization started for.
 */
function readOutcome(tenant: string): {
  outcome: Outcome;
  keySettingsUrl: string | null;
  keyWarnings: boolean;
} | null {
  const value = sessionStorage.getItem("aidash-provider-authorization");
  const link = sessionStorage.getItem(
    "aidash-provider-authorization-key-settings-url",
  );
  const warned = sessionStorage.getItem(
    "aidash-provider-authorization-key-warnings",
  );
  const started = sessionStorage.getItem(tenantKey);
  if (!value || !(outcomes as readonly string[]).includes(value)) return null;
  if (started !== tenant) return null;
  const outcome = value as Outcome;
  // Each of these stored the issued key, so its validation warnings apply.
  const stored =
    outcome === "connected" ||
    outcome === "unbound" ||
    outcome === "reconnected";
  return {
    outcome,
    keySettingsUrl:
      (outcome === "failed" || outcome === "reconnected") &&
      link &&
      keySettingsPattern.test(link)
        ? link
        : null,
    keyWarnings: stored && warned === "1",
  };
}

/** The dashboard path the callback returns to, without a stale outcome. */
function returnPath(): string {
  const url = new URL(window.location.href);
  url.searchParams.delete("provider_authorization");
  url.searchParams.delete("provider_key_settings_url");
  url.searchParams.delete("provider_key_warnings");
  return url.pathname + url.search;
}

const stateTones: Record<string, StatusTone> = {
  active: "success",
  pending: "warning",
  revoked: "danger",
  deleted: "neutral",
};

export function ProviderCredentialsPage({
  access,
}: {
  access: State["access"];
}) {
  const { t } = useI18n();
  const [selected, select] = useState(() =>
    access.kind === "deployment_administrator" ? returningTenant() : "",
  );
  const tenant = access.kind === "subject" ? access.tenant : selected;
  return (
    <div className="grid max-w-5xl min-w-0 gap-6">
      <Notice>{t("providerCredentialsHelp")}</Notice>
      {access.kind === "deployment_administrator" && (
        <form
          className="flex min-w-0 flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            select(
              String(new FormData(event.currentTarget).get("tenant")).trim(),
            );
          }}
        >
          <div className="w-full max-w-xs">
            <Field label={t("tenant")}>
              <Input
                name="tenant"
                required
                maxLength={256}
                defaultValue={selected}
                className="font-mono text-xs"
              />
            </Field>
          </div>
          <Button variant="outline">{t("open")}</Button>
        </form>
      )}
      {tenant && <TenantProviderCredentials key={tenant} tenant={tenant} />}
    </div>
  );
}
function TenantProviderCredentials({ tenant }: { tenant: string }) {
  const { t, locale } = useI18n();
  const client = useQueryClient();
  const path = encodeURIComponent(tenant);
  const [offset, setOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [callback] = useState(() => readOutcome(tenant));
  useEffect(() => {
    // A render can be retried before it commits. Keep the returning Tenant
    // and its outcome available until this page actually mounts.
    if (!sessionStorage.getItem("aidash-provider-authorization")) return;
    sessionStorage.removeItem("aidash-provider-authorization");
    sessionStorage.removeItem("aidash-provider-authorization-key-settings-url");
    sessionStorage.removeItem("aidash-provider-authorization-key-warnings");
    sessionStorage.removeItem(tenantKey);
  }, []);
  const outcome = callback?.outcome;
  // Revocation, deletion and a failed authorization leave a live key at
  // OpenRouter; its settings page is where the user deletes it.
  const [cleanupLink, setCleanupLink] = useState<string | null>(null);
  const rows = useQuery({
    queryKey: ["provider-credentials", tenant, offset],
    queryFn: () => providerCredentialList(path, { offset, limit: 50 }),
    retry: false,
  });
  const bindings = useQuery({
    queryKey: ["provider-credential-bindings", tenant],
    queryFn: () => providerCredentialBindingList(path),
    retry: false,
  });
  const bound = bindings.data?.find(
    (binding) => binding.provider === "openrouter",
  );
  const boundId = bound?.provider_credential_id;
  const boundMetadata = useQuery({
    queryKey: ["provider-credentials", tenant, "bound", boundId],
    queryFn: () => providerCredentialGet(path, boundId!),
    enabled:
      !!boundId && !!rows.data && !rows.data.some((row) => row.id === boundId),
    retry: false,
  });
  const choices = [...(rows.data ?? [])];
  if (
    boundMetadata.data &&
    !choices.some((row) => row.id === boundMetadata.data.id)
  )
    choices.push(boundMetadata.data);
  const refresh = async () => {
    await Promise.all([
      client.invalidateQueries({ queryKey: ["provider-credentials", tenant] }),
      client.invalidateQueries({
        queryKey: ["provider-credential-bindings", tenant],
      }),
    ]);
  };
  const act = async (effect: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await effect();
      await refresh();
    } catch (error) {
      setError(error instanceof Error ? error.message : t("operationFailed"));
    } finally {
      setBusy(false);
    }
  };
  // Provider Authorization: the browser leaves for OpenRouter's consent page
  // and returns through the server's callback; no key passes through here.
  const authorize = (target?: { id: string; revision: number }) =>
    act(async () => {
      const started = await providerAuthorizationStart(path, "openrouter", {
        provider_credential_id: target?.id ?? null,
        expected_revision: target?.revision ?? null,
        return_to: returnPath(),
      });
      sessionStorage.setItem(tenantKey, tenant);
      window.location.assign(started.authorization_url);
    });
  const disabled = rows.error instanceof ApiError && rows.error.status === 404;
  const connected = choices.find(
    (row) => row.id === boundId && row.state === "active",
  );
  // Connect versus Reconnect depends on the bound record's metadata; until it
  // is known, an enabled Connect would mint another key without a target.
  const connectionKnown =
    !!bindings.data &&
    !!rows.data &&
    (!boundId ||
      rows.data.some((row) => row.id === boundId) ||
      !!boundMetadata.data ||
      boundMetadata.isError);
  return disabled ? (
    <Notice role="status">{t("providerCredentialsDisabled")}</Notice>
  ) : (
    <>
      {error && <Alert>{error}</Alert>}
      {rows.error && !disabled && <Alert>{rows.error.message}</Alert>}
      {outcome && (
        <Notice
          role="status"
          tone={
            (outcome === "connected" || outcome === "reconnected") &&
            !callback?.keySettingsUrl &&
            !callback?.keyWarnings
              ? "info"
              : "warning"
          }
        >
          {callback?.keySettingsUrl ? (
            <>
              {t(
                outcome === "reconnected"
                  ? "providerConnectionOutcome_reconnectedPrevious"
                  : "providerConnectionOutcome_failedIssued",
              )}{" "}
              <a
                className="underline"
                href={callback.keySettingsUrl}
                target="_blank"
                rel="noreferrer noopener"
              >
                {t("providerKeySettings")}
              </a>
            </>
          ) : (
            t(`providerConnectionOutcome_${outcome}`)
          )}
          {callback?.keyWarnings && <> {t("providerConnectionKeyWarnings")}</>}
        </Notice>
      )}
      {cleanupLink && (
        <Notice role="status" tone="warning">
          {t("providerCredentialRevokedNotice")}{" "}
          <a
            className="underline"
            href={cleanupLink}
            target="_blank"
            rel="noreferrer noopener"
          >
            {t("providerKeySettings")}
          </a>
        </Notice>
      )}
      <Panel title={t("providerConnections")}>
        <p className="text-xs text-muted-foreground">
          {t("providerConnectionsHelp")}
        </p>
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-surface px-3 py-2">
          <div className="flex min-w-0 items-center gap-2">
            <span className="font-medium">OpenRouter</span>
            <StatusBadge tone={connected ? "success" : "neutral"}>
              {connected
                ? t("providerConnectionConnected")
                : t("providerConnectionNotConnected")}
            </StatusBadge>
            {connected && (
              <span className="font-mono text-xs text-muted-foreground">
                {connected.last4} · {connected.fingerprint}
              </span>
            )}
          </div>
          <div className="flex gap-1.5">
            {connected?.key_settings_url && (
              <Button variant="ghost" size="sm" asChild>
                <a
                  href={connected.key_settings_url}
                  target="_blank"
                  rel="noreferrer noopener"
                >
                  {t("providerKeySettings")}
                </a>
              </Button>
            )}
            <Button
              size="sm"
              variant={connected ? "outline" : "default"}
              disabled={busy || !connectionKnown}
              onClick={() =>
                void authorize(
                  connected
                    ? { id: connected.id, revision: connected.revision }
                    : undefined,
                )
              }
            >
              {connected
                ? t("providerConnectionReconnect")
                : t("providerConnectionConnect")}
            </Button>
          </div>
        </div>
      </Panel>
      <Panel title={t("providerCredentialBindings")}>
        <div className="w-full max-w-md">
          <Field label="OpenRouter">
            <NativeSelect
              aria-label={t("providerCredentialBindings")}
              value={bound?.provider_credential_id ?? ""}
              disabled={busy || !bindings.data}
              onChange={(event) => {
                const id = event.currentTarget.value;
                void act(() =>
                  providerCredentialBindingUpdate(path, "openrouter", {
                    provider_credential_id: id || null,
                    expected_revision: bound?.revision ?? 0,
                  }),
                );
              }}
            >
              <option value="">{t("providerCredentialUnbound")}</option>
              {boundId && !choices.some((row) => row.id === boundId) && (
                <option value={boundId} disabled>
                  {t("providerCredentialCurrentBinding")}
                </option>
              )}
              {choices
                .filter(
                  (row) =>
                    row.state === "active" ||
                    row.id === bound?.provider_credential_id,
                )
                .map((row) => (
                  <option key={row.id} value={row.id}>
                    {row.provider} · {row.last4} · {row.fingerprint} ·{" "}
                    {row.state}
                  </option>
                ))}
            </NativeSelect>
          </Field>
        </div>
        {bindings.error && <Alert>{bindings.error.message}</Alert>}
        {boundMetadata.error && <Alert>{boundMetadata.error.message}</Alert>}
      </Panel>
      <Panel
        title={locale === "ja-JP" ? "保存済みの認証情報" : "Stored credentials"}
      >
        {rows.data?.length === 0 ? (
          <Empty />
        ) : (
          <div className="min-w-0 rounded-md border border-border bg-surface">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("provider")}</TableHead>
                  <TableHead>{t("providerCredentialLast4")}</TableHead>
                  <TableHead>{t("providerCredentialFingerprint")}</TableHead>
                  <TableHead>{t("createdAt")}</TableHead>
                  <TableHead>{t("providerCredentialRotatedAt")}</TableHead>
                  <TableHead>{t("providerCredentialRevokedAt")}</TableHead>
                  <TableHead />
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.data?.map((row) => (
                  <TableRow key={row.id}>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <span className="font-medium">{row.provider}</span>
                        <StatusBadge tone={stateTones[row.state] ?? "neutral"}>
                          {t(`providerCredentialState_${row.state}`)}
                        </StatusBadge>
                      </div>
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      {row.last4}
                    </TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      {row.fingerprint}
                    </TableCell>
                    <TableCell className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                      {row.created_at}
                    </TableCell>
                    <TableCell className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                      {row.rotated_at ?? "—"}
                    </TableCell>
                    <TableCell className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                      {row.revoked_at ?? "—"}
                    </TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1.5">
                        {row.key_settings_url && (
                          <Button variant="ghost" size="sm" asChild>
                            <a
                              href={row.key_settings_url}
                              target="_blank"
                              rel="noreferrer noopener"
                            >
                              {t("providerKeySettings")}
                            </a>
                          </Button>
                        )}
                        {row.state === "active" && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={busy}
                            onClick={() =>
                              void authorize({
                                id: row.id,
                                revision: row.revision,
                              })
                            }
                          >
                            {t("providerConnectionReconnect")}
                          </Button>
                        )}
                        {row.state === "active" && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={busy}
                            onClick={() =>
                              void act(async () => {
                                const revoked = await providerCredentialRevoke(
                                  path,
                                  row.id,
                                  { expected_revision: row.revision },
                                );
                                setCleanupLink(
                                  revoked.key_settings_url ?? null,
                                );
                              })
                            }
                          >
                            {t("revoke")}
                          </Button>
                        )}
                        {row.state !== "deleted" && row.state !== "pending" && (
                          <Button
                            variant="destructive"
                            size="sm"
                            disabled={
                              busy || bound?.provider_credential_id === row.id
                            }
                            onClick={() =>
                              void act(async () => {
                                const deleted = await providerCredentialDelete(
                                  path,
                                  row.id,
                                  { expected_revision: row.revision },
                                );
                                setCleanupLink(
                                  deleted.key_settings_url ?? null,
                                );
                              })
                            }
                          >
                            {t("delete")}
                          </Button>
                        )}
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        <div className="flex justify-end gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={offset === 0 || busy}
            onClick={() => setOffset(Math.max(0, offset - 50))}
          >
            {t("previous")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={(rows.data?.length ?? 0) < 50 || busy}
            onClick={() => setOffset(offset + 50)}
          >
            {t("next")}
          </Button>
        </div>
      </Panel>
    </>
  );
}
