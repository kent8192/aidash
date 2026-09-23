import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, authenticatedFetch } from "./transport";
import { useI18n } from "./ui";

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
  disabled_at: string | null;
};
type Mapping = {
  id: string;
  identity_id: string;
  tenant: string;
  subject: string;
  enabled: boolean;
};
type Grant = { identity_id: string; enabled: boolean; revision: number };

export function DashboardIdentityAdministration() {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const [tenant, setTenant] = useState("");
  const [subject, setSubject] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const registrations = useQuery({
    queryKey: ["dashboard-registrations"],
    queryFn: () => apiFetch<Registration[]>("/api/dashboard/registrations"),
  });
  const identities = useQuery({
    queryKey: ["dashboard-identities"],
    queryFn: () => apiFetch<Identity[]>("/api/dashboard/identities"),
  });
  const mappings = useQuery({
    queryKey: ["dashboard-mappings"],
    queryFn: () => apiFetch<Mapping[]>("/api/dashboard/mappings"),
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
  return (
    <section className="panel">
      <h2>{english ? "Dashboard identities" : "ダッシュボード ID"}</h2>
      <p>
        {english
          ? "Approve a verified identity only for an existing user subject. Operator access is separate."
          : "確認済みの外部 ID を既存の user subject に紐づけます。operator 権限は別に設定します。"}
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {(registrations.data ?? [])
        .filter((request) => request.status === "pending")
        .map((request) => (
          <div className="card" key={request.id}>
            <strong>
              {identity(request.identity_id)?.subject ?? request.identity_id}
            </strong>
            <p>{identity(request.identity_id)?.issuer}</p>
            <small>
              {english ? "Expires" : "期限"}:{" "}
              {new Date(request.expires_at).toLocaleString(locale)}
            </small>
            <label>
              {english ? "Tenant" : "テナント"}
              <input
                value={tenant}
                onChange={(event) => setTenant(event.target.value)}
              />
            </label>
            <label>
              {english ? "Existing user subject" : "既存の user subject"}
              <input
                value={subject}
                onChange={(event) => setSubject(event.target.value)}
              />
            </label>
            <button
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
            </button>
            <button
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
            </button>
          </div>
        ))}
      <h3>{english ? "Mappings" : "紐づけ"}</h3>
      {(mappings.data ?? []).map((mapping) => (
        <div className="card" key={mapping.id}>
          <span>
            {identity(mapping.identity_id)?.subject ?? mapping.identity_id} →{" "}
            {mapping.tenant} / {mapping.subject}
          </span>
          {mapping.enabled && (
            <button
              disabled={busy}
              onClick={() => {
                void act(() =>
                  authenticatedFetch(
                    `/api/dashboard/mappings/${mapping.id}/disable`,
                    { method: "POST" },
                  ),
                );
              }}
            >
              {english ? "Disable" : "無効化"}
            </button>
          )}
        </div>
      ))}
      <h3>{english ? "Operator grants" : "operator 権限"}</h3>
      {(identities.data ?? []).map((item) => {
        const grant = grants.data?.find(
          (entry) => entry.identity_id === item.id,
        );
        const enabled = grant?.enabled ?? false;
        return (
          <div className="card" key={item.id}>
            <span>
              {item.subject} ({item.issuer})
            </span>
            {item.disabled_at && (
              <button
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
              </button>
            )}
            <button
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
            </button>
          </div>
        );
      })}
    </section>
  );
}
