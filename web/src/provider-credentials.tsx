import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Badge, Field, Panel, useI18n } from "./ui";
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

export function ProviderCredentialsPage({
  access,
}: {
  access: State["access"];
}) {
  const { t } = useI18n();
  const [selected, select] = useState("");
  const tenant = access.kind === "subject" ? access.tenant : selected;
  return (
    <section className="authorization-page">
      <h2>{t("providerCredentials")}</h2>
      <p className="notice">{t("providerCredentialsHelp")}</p>
      {access.kind === "operator" && (
        <form
          className="generation-tenant"
          onSubmit={(event) => {
            event.preventDefault();
            select(
              String(new FormData(event.currentTarget).get("tenant")).trim(),
            );
          }}
        >
          <Field label={t("tenant")}>
            <input name="tenant" required maxLength={256} />
          </Field>
          <Button variant="outline">{t("open")}</Button>
        </form>
      )}
      {tenant && <TenantProviderCredentials key={tenant} tenant={tenant} />}
    </section>
  );
}
function TenantProviderCredentials({ tenant }: { tenant: string }) {
  const { t } = useI18n();
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
  return (
    <>
      {disabled ? (
        <p className="notice" role="status">
          {t("providerCredentialsDisabled")}
        </p>
      ) : (
        <>
          {error && <p role="alert">{error}</p>}
          {rows.error && !disabled && <p role="alert">{rows.error.message}</p>}
          <Panel title={t("providerCredentialBindings")}>
            <Field label="OpenRouter">
              <select
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
              </select>
            </Field>
            {bindings.error && <p role="alert">{bindings.error.message}</p>}
            {boundMetadata.error && (
              <p role="alert">{boundMetadata.error.message}</p>
            )}
          </Panel>
          <div className="cards">
            {rows.data?.map((row) => (
              <Panel key={row.id} title={`${row.provider} · ${row.last4}`}>
                <Badge value={t(`providerCredentialState_${row.state}`)} />
                <dl>
                  <dt>{t("providerCredentialFingerprint")}</dt>
                  <dd>{row.fingerprint}</dd>
                  <dt>{t("providerCredentialLast4")}</dt>
                  <dd>{row.last4}</dd>
                  <dt>{t("createdAt")}</dt>
                  <dd>{row.created_at}</dd>
                  <dt>{t("providerCredentialRotatedAt")}</dt>
                  <dd>{row.rotated_at ?? "—"}</dd>
                  <dt>{t("providerCredentialRevokedAt")}</dt>
                  <dd>{row.revoked_at ?? "—"}</dd>
                </dl>
                {row.state === "active" && (
                  <>
                    <Button
                      variant="outline"
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
                  </>
                )}
                {row.state !== "deleted" && row.state !== "pending" && (
                  <Button
                    variant="outline"
                    disabled={busy || bound?.provider_credential_id === row.id}
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
              </Panel>
            ))}
          </div>
          <Button
            variant="outline"
            disabled={offset === 0 || busy}
            onClick={() => setOffset(Math.max(0, offset - 50))}
          >
            {t("previous")}
          </Button>
          <Button
            variant="outline"
            disabled={(rows.data?.length ?? 0) < 50 || busy}
            onClick={() => setOffset(offset + 50)}
          >
            {t("next")}
          </Button>
        </>
      )}
    </>
  );
}
