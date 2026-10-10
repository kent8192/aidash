import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, authenticatedFetch } from "./transport";
import { Panel, useI18n } from "./ui";
import { Alert, Hint, Pager } from "./components/patterns";

type Registration = {
  id: string;
  identity_id: string;
  status: string;
  created_at: string;
  expires_at: string;
};
type Identity = {
  id: string;
  issuer: string;
  subject: string;
  gcip_tenant: string | null;
  verified_email: string | null;
  display_name: string | null;
  disabled_at: string | null;
  display_erased_at: string | null;
};
type Mapping = {
  id: string;
  identity_id: string;
  tenant: string;
  subject: string;
  enabled: boolean;
  revision: number;
};
type Grant = { identity_id: string; enabled: boolean; revision: number };
const MAPPING_PAGE_SIZE = 200;

export function DashboardIdentityAdministration() {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const [tenant, setTenant] = useState("");
  const [subject, setSubject] = useState("");
  const [mappingOffset, setMappingOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const registrations = useQuery({
    queryKey: ["dashboard-registrations"],
    queryFn: () => apiFetch<Registration[]>("/api/dashboard/registrations"),
  });
  const identities = useQuery({
    queryKey: ["dashboard-identities"],
    queryFn: async () => {
      const all: Identity[] = [];
      for (let offset = 0; ; offset += 200) {
        const page = await apiFetch<Identity[]>(
          `/api/dashboard/identities?offset=${offset}`,
        );
        all.push(...page);
        if (page.length < 200) return all;
      }
    },
  });
  const mappings = useQuery({
    queryKey: ["dashboard-mappings", mappingOffset],
    queryFn: () =>
      apiFetch<Mapping[]>(`/api/dashboard/mappings?offset=${mappingOffset}`),
  });
  const grants = useQuery({
    queryKey: ["dashboard-operator-grants"],
    queryFn: () => apiFetch<Grant[]>("/api/dashboard/operator-grants"),
  });
  const identity = (id: string) =>
    identities.data?.find((item) => item.id === id);
  const act = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries({ queryKey: ["dashboard-registrations"] });
      await client.invalidateQueries({ queryKey: ["dashboard-identities"] });
      await client.invalidateQueries({ queryKey: ["dashboard-mappings"] });
      await client.invalidateQueries({
        queryKey: ["dashboard-operator-grants"],
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      await client.invalidateQueries({
        queryKey: ["dashboard-operator-grants"],
      });
    } finally {
      setBusy(false);
    }
  };
  const field = "grid gap-1 text-xs font-medium text-muted-foreground";
  const row = "flex flex-wrap items-center gap-x-3 gap-y-2 py-2.5";
  const group = "text-[11px] font-medium text-faint";
  return (
    <Panel title={english ? "Dashboard identities" : "ダッシュボード ID"}>
      <Hint>
        {english
          ? "Approve a verified identity only for an existing user subject. Operator access is separate."
          : "確認済みの外部 ID を既存の user subject に紐づけます。operator 権限は別に設定します。"}
      </Hint>
      {error && <Alert>{error}</Alert>}
      <ul className="divide-y divide-border">
        {(registrations.data ?? [])
          .filter((request) => request.status === "pending")
          .map((request) => (
            <li
              key={request.id}
              className="grid gap-3 py-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]"
            >
              <div className="grid min-w-0 content-start gap-0.5">
                <strong className="truncate font-mono text-[13px] font-medium">
                  {identity(request.identity_id)?.subject ??
                    request.identity_id}
                </strong>
                <p className="truncate font-mono text-[11px] text-faint">
                  {identity(request.identity_id)?.issuer}
                </p>
                <p className="truncate font-mono text-[11px] text-faint">
                  {identity(request.identity_id)?.gcip_tenant}
                </p>
                <p className="truncate text-xs text-muted-foreground">
                  {[
                    identity(request.identity_id)?.display_name,
                    identity(request.identity_id)?.verified_email,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </p>
                <small className="text-[11px] text-faint">
                  {english ? "Expires" : "期限"}:{" "}
                  <span className="font-mono tabular">
                    {new Date(request.expires_at).toLocaleString(locale)}
                  </span>
                </small>
              </div>
              <div className="grid content-start gap-2 sm:grid-cols-2">
                <label className={field}>
                  {english ? "Tenant" : "テナント"}
                  <Input
                    value={tenant}
                    onChange={(event) => setTenant(event.target.value)}
                  />
                </label>
                <label className={field}>
                  {english ? "Existing user subject" : "既存の user subject"}
                  <Input
                    value={subject}
                    onChange={(event) => setSubject(event.target.value)}
                  />
                </label>
                <div className="flex gap-2 sm:col-span-2">
                  <Button
                    size="sm"
                    disabled={busy || !tenant || !subject}
                    onClick={() => {
                      void act(() =>
                        apiFetch(
                          `/api/dashboard/registrations/${request.id}/approve`,
                          {
                            method: "POST",
                            headers: { "content-type": "application/json" },
                            body: JSON.stringify({ tenant, subject }),
                          },
                        ),
                      );
                    }}
                  >
                    {english ? "Approve mapping" : "紐づけを承認"}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => {
                      void act(() =>
                        apiFetch(
                          `/api/dashboard/registrations/${request.id}/reject`,
                          { method: "POST" },
                        ),
                      );
                    }}
                  >
                    {english ? "Reject" : "却下"}
                  </Button>
                </div>
              </div>
            </li>
          ))}
      </ul>
      <h3 className={group}>{english ? "Mappings" : "紐づけ"}</h3>
      <ul className="divide-y divide-border border-y border-border">
        {(mappings.data ?? []).map((mapping) => (
          <li className={row} key={mapping.id}>
            <span className="min-w-0 flex-1 truncate font-mono text-xs">
              {identity(mapping.identity_id)?.subject ?? mapping.identity_id}{" "}
              <span className="text-faint">→</span> {mapping.tenant} /{" "}
              {mapping.subject}
            </span>
            {mapping.enabled && (
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => {
                  void act(() =>
                    authenticatedFetch(
                      `/api/dashboard/mappings/${mapping.id}/disable`,
                      {
                        method: "POST",
                        headers: { "content-type": "application/json" },
                        body: JSON.stringify({
                          expected_revision: mapping.revision,
                        }),
                      },
                    ),
                  );
                }}
              >
                {english ? "Disable" : "無効化"}
              </Button>
            )}
          </li>
        ))}
      </ul>
      <Pager
        className="justify-start"
        page={mappingOffset / MAPPING_PAGE_SIZE + 1}
        previous={{
          disabled: mappingOffset === 0 || mappings.isFetching,
          go: () =>
            setMappingOffset(Math.max(0, mappingOffset - MAPPING_PAGE_SIZE)),
        }}
        next={{
          disabled:
            (mappings.data?.length ?? 0) < MAPPING_PAGE_SIZE ||
            mappings.isFetching,
          go: () => setMappingOffset(mappingOffset + MAPPING_PAGE_SIZE),
        }}
      />
      <h3 className={group}>{english ? "Operator grants" : "operator 権限"}</h3>
      <ul className="divide-y divide-border border-y border-border">
        {(identities.data ?? []).map((item) => {
          const grant = grants.data?.find(
            (entry) => entry.identity_id === item.id,
          );
          const enabled = grant?.enabled ?? false;
          return (
            <li className={row} key={item.id}>
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="truncate font-mono text-xs">
                  {item.subject} ({item.issuer})
                  {item.gcip_tenant && ` / ${item.gcip_tenant}`}
                </span>
                {(item.display_name || item.verified_email) && (
                  <span className="truncate text-xs text-muted-foreground">
                    {[item.display_name, item.verified_email]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                )}
                {item.display_erased_at && (
                  <span className="truncate text-[11px] text-faint">
                    {english
                      ? "Display attributes erased"
                      : "表示属性を消去済み"}
                    :{" "}
                    <span className="font-mono tabular">
                      {new Date(item.display_erased_at).toLocaleString(locale)}
                    </span>
                  </span>
                )}
              </span>
              {item.disabled_at && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    void act(async () => {
                      await authenticatedFetch(
                        `/api/dashboard/identities/${item.id}/restore`,
                        { method: "POST" },
                      );
                      await client.invalidateQueries({
                        queryKey: ["dashboard-identities"],
                      });
                    });
                  }}
                >
                  {english
                    ? "Verify and restore identity"
                    : "外部 ID を確認して復旧"}
                </Button>
              )}
              {!item.display_erased_at && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    if (
                      !window.confirm(
                        english
                          ? "Permanently erase this identity's email and display name? Later sign-ins will not record them again."
                          : "この外部 ID のメールアドレスと表示名を完全に消去しますか？以後のサインインでも再取得しません。",
                      )
                    )
                      return;
                    void act(() =>
                      authenticatedFetch(
                        `/api/dashboard/identities/${item.id}/display-erasure`,
                        { method: "POST" },
                      ),
                    );
                  }}
                >
                  {english ? "Erase display attributes" : "表示属性を消去"}
                </Button>
              )}
              <Button
                variant={enabled ? "destructive" : "outline"}
                size="sm"
                disabled={busy || (item.disabled_at !== null && !enabled)}
                onClick={() => {
                  void act(() =>
                    authenticatedFetch(
                      `/api/dashboard/identities/${item.id}/operator-grant`,
                      {
                        method: "POST",
                        headers: { "content-type": "application/json" },
                        body: JSON.stringify({
                          enabled: !enabled,
                          expected_revision: grant?.revision ?? 0,
                        }),
                      },
                    ),
                  );
                }}
              >
                {enabled
                  ? english
                    ? "Revoke operator"
                    : "operator 権限を解除"
                  : english
                    ? "Grant operator"
                    : "operator 権限を付与"}
              </Button>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}
