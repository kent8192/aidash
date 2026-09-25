import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { RemoteExecutionStatus } from "../generated/models";
import { apiFetch } from "../transport";
import { Badge, useI18n } from "../ui";

export function RemoteExecutions({ task }: { task: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const clock = useQuery({
    queryKey: ["remote-execution-clock"],
    queryFn: () => Date.now(),
    refetchInterval: 1000,
  });
  const status = useQuery({
    queryKey: ["remote-executions", task],
    queryFn: () =>
      apiFetch<RemoteExecutionStatus[]>(`/api/tasks/${task}/remote-executions`),
    retry: false,
    refetchInterval: 5000,
  });
  async function act(id: string, action: string) {
    setBusy(id);
    setError("");
    try {
      const endpoint = action === "activate" ? "activate" : "control";
      await apiFetch(`/api/tasks/${task}/remote-grants/${id}/${endpoint}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(endpoint === "control" ? { action } : {}),
      });
      await client.invalidateQueries();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }
  if (status.isPending || (!status.isError && status.data?.length === 0))
    return null;
  return (
    <section aria-label={ja ? "遠隔実行" : "Remote execution"}>
      <h3>{ja ? "遠隔実行" : "Remote execution"}</h3>
      {status.isError && <p role="alert">{status.error.message}</p>}
      {error && <p role="alert">{error}</p>}
      <button type="button" onClick={() => void status.refetch()}>
        {ja ? "状態を再読み込み" : "Refresh status"}
      </button>
      {status.data?.map(({ grant, execution, unavailable }) => {
        const expired =
          Date.parse(grant.expires_at) <=
          (clock.data ?? Number.POSITIVE_INFINITY);
        const live = !grant.revoked && !expired;
        const terminal =
          execution &&
          ["COMPLETED", "FAILED", "CANCELLED"].includes(execution.phase);
        return (
          <article className="detail-section" key={grant.id}>
            <p>{grant.node_id}</p>
            <p>
              {grant.agent.id}@{grant.agent.version}
            </p>
            <dl>
              <dt>{ja ? "許可" : "Grant"}</dt>
              <dd>
                {grant.revoked
                  ? ja
                    ? "取り消し済み"
                    : "Revoked"
                  : expired
                    ? ja
                      ? "期限切れ"
                      : "Expired"
                    : ja
                      ? "有効"
                      : "Valid"}
                <code>{grant.id}</code>
              </dd>
              <dt>{ja ? "有効期限" : "Expires"}</dt>
              <dd>{new Date(grant.expires_at).toLocaleString(locale)}</dd>
              {execution && (
                <>
                  <dt>{ja ? "受信・実行 ID" : "Admission / run ID"}</dt>
                  <dd>
                    <code>{execution.admission_id}</code>
                  </dd>
                  <dt>{ja ? "実行状態" : "Execution state"}</dt>
                  <dd>
                    <Badge value={execution.phase} />
                    <Badge value={execution.control} />
                  </dd>
                </>
              )}
            </dl>
            {unavailable && (
              <p role="status">
                {ja
                  ? "相手ノードに接続できません。再読み込みで状態を確認できます。"
                  : "The destination is unavailable. Refresh to reconcile its durable state."}
              </p>
            )}
            {execution?.error && (
              <p role="alert">
                {ja
                  ? "実行の確認が必要です。現在の権限と接続を確認してから再開してください。"
                  : execution.error}
              </p>
            )}
            {!terminal && (
              <div className="button-row">
                {live && (!execution || execution.phase === "ADMITTED") && (
                  <button
                    disabled={busy === grant.id}
                    onClick={() => void act(grant.id, "activate")}
                  >
                    {ja ? "起動を再試行" : "Retry activation"}
                  </button>
                )}
                {execution && execution.phase !== "ADMITTED" && (
                  <>
                    {live && execution.control === "PAUSED" && (
                      <button
                        disabled={busy === grant.id}
                        onClick={() => void act(grant.id, "resume")}
                      >
                        {ja
                          ? "権限を再確認して再開"
                          : "Recheck authority and resume"}
                      </button>
                    )}
                    {execution.control === "ACTIVE" && (
                      <button
                        disabled={busy === grant.id}
                        onClick={() => void act(grant.id, "pause")}
                      >
                        {ja ? "一時停止" : "Pause"}
                      </button>
                    )}
                    {execution.control !== "CANCELLED" && (
                      <button
                        disabled={busy === grant.id}
                        onClick={() => void act(grant.id, "cancel")}
                      >
                        {ja ? "実行を中止" : "Cancel execution"}
                      </button>
                    )}
                  </>
                )}
              </div>
            )}
            {live &&
              !terminal &&
              execution &&
              execution.phase !== "ADMITTED" && (
                <RemoteMessage task={task} grant={grant.id} />
              )}
          </article>
        );
      })}
    </section>
  );
}

function RemoteMessage({ task, grant }: { task: string; grant: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [content, setContent] = useState("");
  const [pending, setPending] = useState<{
    id: string;
    content: string;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [accepted, setAccepted] = useState(false);
  async function send() {
    const input = pending ?? { id: crypto.randomUUID(), content };
    setPending(input);
    setBusy(true);
    setError("");
    setAccepted(false);
    try {
      await apiFetch(`/api/tasks/${task}/remote-grants/${grant}/messages`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
      });
      setPending(null);
      setContent("");
      setAccepted(true);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
    >
      <label>
        {ja ? "遠隔 Agent への追加指示" : "Instruction for the remote Agent"}
        <textarea
          required
          maxLength={16000}
          value={content}
          disabled={pending !== null}
          onChange={(event) => setContent(event.target.value)}
        />
      </label>
      {error && <p role="alert">{error}</p>}
      {accepted && (
        <p role="status">
          {ja
            ? "指示の受理を確認しました。"
            : "Instruction acceptance confirmed."}
        </p>
      )}
      <button disabled={busy || !content.trim()}>
        {pending
          ? ja
            ? "同じ指示の受理を再確認"
            : "Retry the same instruction"
          : ja
            ? "追加指示を送る"
            : "Send instruction"}
      </button>
    </form>
  );
}
