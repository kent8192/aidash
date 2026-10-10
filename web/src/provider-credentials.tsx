import { useState } from "react";
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
} from "./generated/aidash";

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
  const [selected, select] = useState("");
  const tenant = access.kind === "subject" ? access.tenant : selected;
  return (
    <div className="grid max-w-5xl min-w-0 gap-6">
      <Notice>{t("providerCredentialsHelp")}</Notice>
      {access.kind === "operator" && (
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
  const disabled = rows.error instanceof ApiError && rows.error.status === 404;
  return disabled ? (
    <Notice role="status">{t("providerCredentialsDisabled")}</Notice>
  ) : (
    <>
      {error && <Alert>{error}</Alert>}
      {rows.error && !disabled && <Alert>{rows.error.message}</Alert>}
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
                        {row.state === "active" && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={busy}
                            onClick={() =>
                              void act(() =>
                                providerCredentialRevoke(path, row.id, {
                                  expected_revision: row.revision,
                                }),
                              )
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
                              void act(() =>
                                providerCredentialDelete(path, row.id, {
                                  expected_revision: row.revision,
                                }),
                              )
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
