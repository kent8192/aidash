import { useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import { post } from "./client";
import type { ApprovalCard as Approval } from "../generated/models";
function Card({ item }: { item: Approval }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [runGrant, setRunGrant] = useState(false);
  const [minutes, setMinutes] = useState(15);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const action = async (choice: string) => {
    setBusy(true);
    setError("");
    try {
      if (choice === "revoke")
        await post(
          `/capabilities/${item.kind === "grant" ? "grants" : "approvals"}/${item.id}/revoke`,
          {
            idempotency_key: crypto.randomUUID(),
            expected_revision: item.revision,
          },
        );
      else
        await post(`/capabilities/approvals/${item.id}/decide`, {
          idempotency_key: crypto.randomUUID(),
          expected_revision: item.revision,
          choice,
          targets: runGrant ? item.targets : null,
          expires_at: runGrant
            ? new Date(Date.now() + minutes * 60000).toISOString()
            : null,
        });
      await client.invalidateQueries({ queryKey: ["core-approvals"] });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <article>
      <strong>{ja ? "外部接続の承認" : "Outbound request"}</strong>
      <p>{item.targets.join(", ")}</p>
      <small>
        {ja ? "要求者" : "Requester"}: {item.requester} ·{" "}
        {ja ? "承認者" : "Approver"}:{" "}
        {item.approver ??
          (ja
            ? "承認できる人がいないため実行できません"
            : "Blocked: no eligible approver")}
      </small>
      <p role="status">
        {item.state}
        {item.expires_at && (
          <> · {new Date(item.expires_at).toLocaleString()}</>
        )}
      </p>
      {item.state === "pending" && (
        <>
          <label className="check">
            <input
              type="checkbox"
              checked={runGrant}
              onChange={(e) => setRunGrant(e.target.checked)}
            />
            {ja
              ? "この Run で同じ接続先を期限付きで許可"
              : "Grant the same targets for this Run until expiry"}
          </label>
          {runGrant && (
            <Field
              label={
                ja ? "有効期間（分、最大60）" : "Duration (minutes, up to 60)"
              }
            >
              <input
                type="number"
                min={1}
                max={60}
                value={minutes}
                onChange={(e) =>
                  setMinutes(Math.max(1, Math.min(60, Number(e.target.value))))
                }
              />
            </Field>
          )}
          <div className="core-inline">
            <button
              type="button"
              disabled={busy}
              onClick={() => void action(runGrant ? "allow_run" : "allow_once")}
            >
              {runGrant
                ? ja
                  ? "期限付きで許可"
                  : "Allow for this Run"
                : ja
                  ? "今回だけ許可"
                  : "Allow once"}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void action("deny")}
            >
              {ja ? "拒否" : "Deny"}
            </button>
          </div>
        </>
      )}
      {["active", "approved", "pending", "attempted"].includes(item.state) && (
        <button
          type="button"
          disabled={busy}
          onClick={() => void action("revoke")}
        >
          {ja ? "許可を取り消す" : "Revoke"}
        </button>
      )}
      {error && <p role="alert">{error}</p>}
    </article>
  );
}
export function CapabilityApprovals({ area }: { area?: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const query = useInfiniteQuery({
    queryKey: ["core-approvals"],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{ items: Approval[]; next_cursor: string | null }>(
        `/api/capabilities/approvals${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const items =
    query.data?.pages
      .flatMap((p) => p.items)
      .filter((i) => !area || i.area_id === area) ?? [];
  return (
    <details
      className="core-panel"
      open={items.some((i) => i.state === "pending")}
    >
      <summary>
        {ja ? "承認と接続許可" : "Approvals and grants"} ({items.length})
      </summary>
      {query.isError && <p role="alert">{query.error.message}</p>}
      {items.map((item) => (
        <Card key={item.id} item={item} />
      ))}
      {query.hasNextPage && (
        <button type="button" onClick={() => void query.fetchNextPage()}>
          {ja ? "さらに表示" : "Load more"}
        </button>
      )}
    </details>
  );
}
