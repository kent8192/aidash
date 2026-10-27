import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { OAuthConnectionMetadata } from "../generated/models";
import { apiFetch, dashboardContext } from "../transport";
import { Field, Panel, useI18n } from "../ui";
import { Alert, Hint, Notice } from "../components/patterns";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { connect } from "./connect";
import { ConnectionStep, connectionError, outcomeMessage } from "./install";

export function Connections({
  tenant,
  canAdminister,
}: {
  tenant: string;
  canAdminister: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const cache = useQueryClient();
  const [member, setMember] = useState("");
  const [selectedMember, setSelectedMember] = useState("");
  const [resource, setResource] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const base = `/api/tenants/${encodeURIComponent(tenant)}`;
  const key = ["oauth-connections", dashboardContext(), tenant];
  const own = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<OAuthConnectionMetadata[]>(`${base}/connections`),
    retry: false,
  });
  const members = useQuery({
    queryKey: [...key, "member", selectedMember],
    queryFn: () =>
      apiFetch<OAuthConnectionMetadata[]>(
        `${base}/members/${encodeURIComponent(selectedMember)}/connections`,
      ),
    enabled: canAdminister && !!selectedMember,
    retry: false,
  });
  async function act(action: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await action();
      await cache.invalidateQueries({ queryKey: ["oauth-connections"] });
    } catch (reason) {
      setError(connectionError(reason, ja));
    } finally {
      setBusy(false);
    }
  }
  function list(rows: OAuthConnectionMetadata[] | undefined, subject?: string) {
    return (
      <ul className="divide-y divide-border">
        {rows?.map((item) => (
          <li key={item.id} className="grid gap-2 py-3">
            <span className="break-all font-mono text-sm">{item.resource}</span>
            <div className="flex flex-wrap gap-2">
              <Badge
                tone={
                  item.state === "active"
                    ? "success"
                    : item.state === "needs_reconnection"
                      ? "warning"
                      : "neutral"
                }
              >
                {item.state === "needs_reconnection"
                  ? ja
                    ? "再接続が必要"
                    : "Needs reconnection"
                  : item.state === "active"
                    ? ja
                      ? "接続済み"
                      : "Active"
                    : ja
                      ? "接続待ち"
                      : "Pending"}
              </Badge>
            </div>
            <span className="text-xs">
              {ja ? "スコープ" : "Scopes"}: {item.scopes.join(", ") || "—"}
            </span>
            <div className="flex gap-2">
              {!subject && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() =>
                    void act(async () => {
                      const outcome = await connect({
                        tenant,
                        resource: item.resource,
                        scopes: item.scopes,
                      });
                      setMessage(outcomeMessage(outcome, ja));
                    })
                  }
                >
                  {ja ? "再接続" : "Reconnect"}
                </Button>
              )}
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() =>
                  void act(() =>
                    apiFetch(
                      `${base}/${subject ? `members/${encodeURIComponent(subject)}/` : ""}connections/${item.id}`,
                      { method: "DELETE" },
                    ),
                  )
                }
              >
                {subject
                  ? ja
                    ? "強制切断"
                    : "Force disconnect"
                  : ja
                    ? "切断"
                    : "Disconnect"}
              </Button>
            </div>
          </li>
        ))}
      </ul>
    );
  }
  return (
    <Panel title="Connections">
      <Hint>
        {ja
          ? "自分のアカウントで接続した MCP サーバーです。トークンは表示されません。"
          : "MCP servers connected with your own account. Tokens are never displayed."}
      </Hint>
      {message && <Notice>{message}</Notice>}
      {(error || own.isError) && <Alert>{error || String(own.error)}</Alert>}
      {list(own.data)}
      {own.data?.length === 0 && (
        <Hint>{ja ? "接続はありません。" : "No connections yet."}</Hint>
      )}
      <Field label={ja ? "MCP サーバー" : "MCP server"}>
        <Input
          type="url"
          value={resource}
          onChange={(event) => setResource(event.target.value)}
          placeholder="https://mcp.example.com/mcp"
        />
      </Field>
      <ConnectionStep tenant={tenant} resource={resource} />
      {canAdminister && (
        <div className="grid gap-3 border-t border-border pt-3">
          <Hint>
            {ja
              ? "メンバーの接続情報を確認して強制切断できます。トークンは取得できません。"
              : "View a member's connection metadata and force disconnect. Tokens remain private."}
          </Hint>
          <Field label={ja ? "メンバーの Subject" : "Member subject"}>
            <Input
              value={member}
              onChange={(event) => setMember(event.target.value)}
            />
          </Field>
          <Button
            type="button"
            variant="outline"
            disabled={!member.trim()}
            onClick={() => setSelectedMember(member.trim())}
          >
            {ja ? "接続を表示" : "View connections"}
          </Button>
          {members.isError && <Alert>{String(members.error)}</Alert>}
          {list(members.data, selectedMember)}
        </div>
      )}
    </Panel>
  );
}
