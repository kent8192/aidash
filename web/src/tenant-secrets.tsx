import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import { Panel, Field, useI18n } from "./ui";
import { Alert, Hint } from "./components/patterns";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";

type Metadata = {
  name: string;
  hosts: string[];
  fingerprint: string;
  last4: string;
  revision: number;
};

/** A package receives names from this Tenant, never the stored values. */
export function TenantSecretAssignments({
  tenant,
  slots,
  value,
  onChange,
}: {
  tenant: string;
  slots: string[];
  value: Record<string, string>;
  onChange: (value: Record<string, string>) => void;
}) {
  const { locale } = useI18n();
  const query = useQuery({
    queryKey: ["tenant-secrets", tenant],
    queryFn: () =>
      apiFetch<Metadata[]>(
        `/api/tenants/${encodeURIComponent(tenant)}/secrets`,
      ),
    enabled: slots.length > 0,
    retry: false,
  });
  return (
    <>
      {query.isError && <Alert>{String(query.error)}</Alert>}
      {slots.map((slot) => (
        <Field key={slot} label={`Tenant Secret · ${slot}`}>
          <NativeSelect
            required
            value={value[slot] ?? ""}
            onChange={(event) =>
              onChange({ ...value, [slot]: event.target.value })
            }
          >
            <option value="">
              {locale === "ja-JP"
                ? "Tenant Secret を選択"
                : "Choose a Tenant Secret"}
            </option>
            {value[slot] &&
              !query.data?.some((item) => item.name === value[slot]) && (
                <option value={value[slot]}>{value[slot]}</option>
              )}
            {query.data?.map((item) => (
              <option key={item.name} value={item.name}>
                {item.name} · {item.hosts.join(", ")}
              </option>
            ))}
          </NativeSelect>
        </Field>
      ))}
    </>
  );
}

/** Values live only in the active password field and the write request. */
export function TenantSecrets({
  tenant,
  canManage,
}: {
  tenant: string;
  canManage: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const key = ["tenant-secrets", tenant];
  const url = `/api/tenants/${encodeURIComponent(tenant)}/secrets`;
  const query = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<Metadata[]>(url),
    retry: false,
  });
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [hosts, setHosts] = useState("");
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function write(method: "PUT" | "DELETE", item?: Metadata) {
    if (busy) return;
    setBusy(true);
    setError("");
    const body =
      method === "DELETE"
        ? { expected_revision: item!.revision }
        : {
            expected_revision: revision,
            hosts: hosts.split(/[\s,]+/).filter(Boolean),
            ...(value ? { value } : {}),
          };
    setValue("");
    try {
      await apiFetch(`${url}/${encodeURIComponent(item?.name ?? name)}`, {
        method,
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      setName("");
      setHosts("");
      setRevision(0);
      await client.invalidateQueries({ queryKey: key });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Panel title="Tenant Secrets">
      <Hint>
        {ja
          ? "値は書き込み専用です。登録したホストにだけ送信できます。値の変更はツールのバージョンを変えません。"
          : "Values are write-only and can be sent only to their recorded hosts. Replacing a value does not change any tool version."}
      </Hint>
      {query.isError && <Alert>{String(query.error)}</Alert>}
      <ul className="divide-y divide-border">
        {query.data?.map((item) => (
          <li
            key={item.name}
            className="flex flex-wrap items-center gap-3 py-3"
          >
            <span className="font-mono text-sm">{item.name}</span>
            <span className="text-xs">
              {item.hosts.join(", ")} · {item.fingerprint} · …{item.last4}
            </span>
            {canManage && (
              <>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    setName(item.name);
                    setHosts(item.hosts.join(", "));
                    setRevision(item.revision);
                    setValue("");
                  }}
                >
                  {ja ? "変更" : "Edit"}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => void write("DELETE", item)}
                >
                  {ja ? "削除" : "Delete"}
                </Button>
              </>
            )}
          </li>
        ))}
      </ul>
      {canManage && (
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void write("PUT");
          }}
        >
          <Field label={ja ? "名前" : "Name"}>
            <Input
              value={name}
              disabled={busy || revision > 0}
              onChange={(event) => setName(event.target.value)}
              pattern="[a-z0-9][a-z0-9_-]{0,63}"
              required
            />
          </Field>
          <Field label={ja ? "送信を許可するホスト" : "Allowed hosts"}>
            <Input
              value={hosts}
              disabled={busy}
              onChange={(event) => setHosts(event.target.value)}
              placeholder="api.example.com"
              required
            />
          </Field>
          <Field
            label={
              ja
                ? "値（変更しない場合は空欄）"
                : "Value (leave empty to keep the current value)"
            }
          >
            <Input
              type="password"
              autoComplete="new-password"
              value={value}
              disabled={busy}
              onChange={(event) => setValue(event.target.value)}
              required={revision === 0}
            />
          </Field>
          {error && <Alert>{error}</Alert>}
          <div className="flex gap-2">
            <Button type="submit" disabled={busy}>
              {ja ? "保存" : "Save"}
            </Button>
            {revision > 0 && (
              <Button
                variant="ghost"
                type="button"
                disabled={busy}
                onClick={() => {
                  setName("");
                  setHosts("");
                  setValue("");
                  setRevision(0);
                }}
              >
                {ja ? "キャンセル" : "Cancel"}
              </Button>
            )}
          </div>
        </form>
      )}
    </Panel>
  );
}

export function SecretDisclosure({
  entry,
}: {
  entry: { kind: string; config: unknown };
}) {
  const { locale } = useI18n();
  const config = entry.config as Record<string, unknown>;
  const transport =
    typeof config.transport === "object"
      ? (config.transport as Record<string, unknown>)
      : config;
  const secret =
    entry.kind === "external_service"
      ? config.secret
      : entry.kind === "tool"
        ? transport.secret
        : undefined;
  const endpoint =
    entry.kind === "external_service" ? config.base_url : transport.endpoint;
  if (!secret && entry.kind !== "external_service") return null;
  let host: string;
  try {
    host = new URL(String(endpoint)).host;
  } catch {
    host = String(endpoint);
  }
  return (
    <Hint>
      <strong>
        {String(secret ?? "External service")} → {host}
      </strong>
      <br />
      {locale === "ja-JP"
        ? "この接続には、データの保存禁止や Usage Records など Aidash の保証は適用されません。Sandboxed code は Working Area 内の Tenant データを含め、読み取れる情報を送信できます。リクエストごとの承認はありません。"
        : "Aidash's guarantees, including zero data retention and Usage Records, do not apply to these calls. Sandboxed code may send anything it can read, including Tenant data in the Working Area. Individual requests are not approved."}
    </Hint>
  );
}
