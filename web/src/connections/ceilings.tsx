import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ConnectionScopeCeiling } from "../generated/models";
import { apiFetch, dashboardContext } from "../transport";
import { Field, Panel, useI18n } from "../ui";
import { Alert, Hint } from "../components/patterns";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { canonicalResource, resourceKey } from "./scopes";

export function ScopeCeilings({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [input, setInput] = useState("");
  const [resource, setResource] = useState<string | null>(null);
  return (
    <Panel
      title={ja ? "MCP サーバーのスコープ上限" : "MCP server scope ceilings"}
    >
      <Hint>
        {ja
          ? "サーバーごとに、各メンバーが同意できるスコープの上限を設定します。空の上限は全スコープを禁止し、上限を削除すると制限がなくなります。"
          : "Limit the scopes each person may consent to for a server. An empty ceiling permits no scopes; removing a ceiling removes the limit."}
      </Hint>
      <Field label={ja ? "MCP サーバー" : "MCP server"}>
        <Input
          type="url"
          value={input}
          onChange={(event) => setInput(event.target.value)}
        />
      </Field>
      <Button
        type="button"
        variant="outline"
        disabled={!canonicalResource(input)}
        onClick={() => setResource(canonicalResource(input))}
      >
        {ja ? "上限を編集" : "Edit ceiling"}
      </Button>
      {resource && (
        <Ceiling key={resource} tenant={tenant} resource={resource} ja={ja} />
      )}
    </Panel>
  );
}
function Ceiling({
  tenant,
  resource,
  ja,
}: {
  tenant: string;
  resource: string;
  ja: boolean;
}) {
  const cache = useQueryClient();
  const key = ["scope-ceiling", dashboardContext(), tenant, resource];
  const url = `/api/tenants/${encodeURIComponent(tenant)}/connection-scope-ceilings/${resourceKey(resource)}`;
  const query = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<ConnectionScopeCeiling | null>(url),
    retry: false,
  });
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function write(remove: boolean) {
    setBusy(true);
    setError("");
    try {
      await apiFetch(url, {
        method: remove ? "DELETE" : "PUT",
        headers: { "content-type": "application/json" },
        ...(remove
          ? {}
          : {
              body: JSON.stringify({
                scopes: (draft ?? query.data?.scopes.join(" ") ?? "")
                  .split(/\s+/)
                  .filter(Boolean),
                expected_revision: query.data?.revision ?? 0,
              }),
            }),
      });
      setDraft(null);
      await cache.invalidateQueries({ queryKey: key });
      await cache.invalidateQueries({ queryKey: ["connection-probe"] });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="grid gap-3">
      <span className="break-all font-mono text-xs">{resource}</span>
      {query.isError && <Alert>{String(query.error)}</Alert>}
      {!query.isPending && !query.isError && (
        <>
          <Hint>
            {query.data
              ? ja
                ? "上限が設定されています"
                : "A ceiling is configured"
              : ja
                ? "上限はありません"
                : "No ceiling configured"}
          </Hint>
          <Field
            label={
              ja
                ? "許可するスコープ（空白区切り）"
                : "Allowed scopes (space separated)"
            }
          >
            <Input
              value={draft ?? query.data?.scopes.join(" ") ?? ""}
              disabled={busy}
              onChange={(event) => setDraft(event.target.value)}
            />
          </Field>
          {error && <Alert>{error}</Alert>}
          <div className="flex gap-2">
            <Button
              type="button"
              disabled={busy}
              onClick={() => void write(false)}
            >
              {ja ? "保存" : "Save"}
            </Button>
            {query.data && (
              <Button
                type="button"
                variant="ghost"
                disabled={busy}
                onClick={() => void write(true)}
              >
                {ja ? "上限を削除" : "Remove ceiling"}
              </Button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
