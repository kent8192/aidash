import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { OAuthClientMetadata, SecretOwnership } from "../generated/models";
import { apiFetch, dashboardContext } from "../transport";
import { Field, Panel, useI18n } from "../ui";
import { Alert, Hint } from "../components/patterns";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { NativeSelect } from "../components/ui/native-select";

export function OAuthClients({
  tenant,
  canUse,
  canManage,
}: {
  tenant: string;
  canUse: boolean;
  canManage: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const cache = useQueryClient();
  const key = ["oauth-clients", dashboardContext(), tenant];
  const url = `/api/tenants/${encodeURIComponent(tenant)}/oauth-clients`;
  const query = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<OAuthClientMetadata[]>(url),
    retry: false,
  });
  const [editing, setEditing] = useState<OAuthClientMetadata | null>(null);
  const [owner, setOwner] = useState<SecretOwnership>(canUse ? "me" : "tenant");
  const [issuer, setIssuer] = useState("");
  const [clientId, setClientId] = useState("");
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  function reset() {
    setEditing(null);
    setIssuer("");
    setClientId("");
    setSecret("");
  }
  async function write(remove?: OAuthClientMetadata) {
    if (busy) return;
    setBusy(true);
    setError("");
    const body = remove
      ? undefined
      : JSON.stringify({
          owner,
          issuer,
          client_id: clientId,
          ...(secret ? { client_secret: secret } : {}),
          expected_revision: editing?.revision ?? 0,
        });
    setSecret("");
    try {
      await apiFetch(
        `${url}${remove || editing ? `/${(remove ?? editing)!.id}` : ""}`,
        {
          method: remove ? "DELETE" : "PUT",
          headers: { "content-type": "application/json" },
          body,
        },
      );
      reset();
      await cache.invalidateQueries({ queryKey: key });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Panel title="OAuth Clients">
      <Hint>
        {ja
          ? "自動登録に対応しない認証サーバー用です。個人の登録が Tenant の登録より優先されます。シークレットは書き込み専用です。"
          : "For authorization servers without dynamic registration. Your own registration takes precedence over the Tenant's. Secrets are write-only."}
      </Hint>
      {query.isError && <Alert>{String(query.error)}</Alert>}
      <ul className="divide-y divide-border">
        {query.data?.map((item) => (
          <li key={item.id} className="flex flex-wrap items-center gap-3 py-3">
            <span className="break-all font-mono text-xs">
              {item.issuer} · {item.client_id}
            </span>
            <Badge variant="outline">
              {item.owner === "me" ? (ja ? "個人" : "Mine") : "Tenant"}
            </Badge>
            <span className="text-xs">
              {item.has_secret
                ? ja
                  ? "シークレット登録済み"
                  : "Secret stored"
                : ja
                  ? "シークレットなし"
                  : "No secret"}
            </span>
            {(item.owner === "me" ? canUse : canManage) && (
              <>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    setEditing(item);
                    setOwner(item.owner);
                    setIssuer(item.issuer);
                    setClientId(item.client_id);
                    setSecret("");
                  }}
                >
                  {ja ? "変更" : "Edit"}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => void write(item)}
                >
                  {ja ? "削除" : "Delete"}
                </Button>
              </>
            )}
          </li>
        ))}
      </ul>
      <form
        className="grid gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          void write();
        }}
      >
        <Field label={ja ? "所有者" : "Owner"}>
          <NativeSelect
            value={owner}
            disabled={busy || !!editing}
            onChange={(event) =>
              setOwner(event.target.value as SecretOwnership)
            }
          >
            {canUse && <option value="me">{ja ? "個人" : "Mine"}</option>}
            {canManage && <option value="tenant">Tenant</option>}
          </NativeSelect>
        </Field>
        <Field label="Issuer">
          <Input
            type="url"
            required
            readOnly={!!editing}
            value={issuer}
            disabled={busy}
            onChange={(event) => setIssuer(event.target.value)}
          />
        </Field>
        <Hint>
          {ja
            ? "Issuer は登録後に変更できません（409 oauth_client_issuer_immutable）。変更する場合は新しく登録してください。"
            : "Issuer cannot change after creation (409 oauth_client_issuer_immutable). Create a new registration for a different issuer."}
        </Hint>
        <Field label="Client ID">
          <Input
            required
            value={clientId}
            disabled={busy}
            onChange={(event) => setClientId(event.target.value)}
          />
        </Field>
        <Field
          label={
            ja
              ? "Client secret（空欄で現在の値を保持。Client ID を変更する場合は必須）"
              : "Client secret (leave empty to keep the current value; required when changing the Client ID)"
          }
        >
          <Input
            type="password"
            autoComplete="new-password"
            value={secret}
            disabled={busy}
            onChange={(event) => setSecret(event.target.value)}
          />
        </Field>
        {error && <Alert>{error}</Alert>}
        <div className="flex gap-2">
          <Button type="submit" disabled={busy}>
            {ja ? "保存" : "Save"}
          </Button>
          {editing && (
            <Button
              type="button"
              variant="ghost"
              disabled={busy}
              onClick={reset}
            >
              {ja ? "キャンセル" : "Cancel"}
            </Button>
          )}
        </div>
      </form>
    </Panel>
  );
}
