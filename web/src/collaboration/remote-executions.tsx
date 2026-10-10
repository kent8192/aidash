import { Button } from "../components/ui/button";
import { Badge as ToneBadge } from "../components/ui/badge";
import { Textarea } from "../components/ui/textarea";
import { RemoteMemoryStatus, RemoteFollowUp } from "./remote-memory";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { RemoteExecutionStatus } from "../generated/models";
import { apiFetch } from "../transport";
import { Alert, Facts, Hint, formClass } from "../components/patterns";
import { Badge, Field, useI18n } from "../ui";

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
    <section
      aria-label={ja ? "遠隔実行" : "Remote execution"}
      className="grid min-w-0 gap-3 border-t border-border pt-4"
    >
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
        <h3 className="text-[13px] font-semibold text-foreground">
          {ja ? "遠隔実行" : "Remote execution"}
        </h3>
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={() => void status.refetch()}
        >
          {ja ? "状態を再読み込み" : "Refresh status"}
        </Button>
      </div>
      {status.isError && <Alert>{status.error.message}</Alert>}
      {error && <Alert>{error}</Alert>}
      <div className="grid min-w-0 divide-y divide-border border-y border-border">
        {(!status.isError ? status.data : undefined)?.map(
          ({ grant, execution, unavailable, semantic, human_requests }) => {
            const expired =
              Date.parse(grant.expires_at) <=
              (clock.data ?? Number.POSITIVE_INFINITY);
            const live = !grant.revoked && !expired;
            const terminal =
              execution &&
              ["COMPLETED", "FAILED", "CANCELLED"].includes(execution.phase);
            return (
              <article className="grid min-w-0 gap-3 py-3" key={grant.id}>
                <div className="grid min-w-0 gap-0.5">
                  <p className="min-w-0 font-mono text-xs text-foreground [overflow-wrap:anywhere]">
                    {grant.node_id}
                  </p>
                  <p className="min-w-0 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]">
                    {grant.agent.id}@{grant.agent.version}
                  </p>
                </div>
                <Facts
                  items={[
                    [
                      ja ? "許可" : "Grant",
                      <span className="flex min-w-0 flex-wrap items-center gap-2">
                        <ToneBadge
                          tone={
                            grant.revoked || expired ? "warning" : "success"
                          }
                        >
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
                        </ToneBadge>
                        <code className="min-w-0 font-mono text-xs text-faint [overflow-wrap:anywhere]">
                          {grant.id}
                        </code>
                      </span>,
                    ],
                    [
                      ja ? "有効期限" : "Expires",
                      new Date(grant.expires_at).toLocaleString(locale),
                      true,
                    ],
                    ...(execution
                      ? ([
                          [
                            ja ? "受信・実行 ID" : "Admission / run ID",
                            execution.admission_id,
                            true,
                          ],
                          [
                            ja ? "実行状態" : "Execution state",
                            <span className="flex flex-wrap items-center gap-1.5">
                              <Badge value={execution.phase} />
                              <Badge value={execution.control} />
                            </span>,
                          ],
                        ] as const)
                      : []),
                  ]}
                />
                {live &&
                  human_requests?.map((request) => (
                    <RemoteHuman
                      key={request.id}
                      task={task}
                      grant={grant.id}
                      request={request}
                    />
                  ))}
                {semantic && (
                  <RemoteMemoryStatus
                    status={semantic}
                    provenanceUrl={`/api/tasks/${task}/remote-grants/${grant.id}/semantic`}
                  />
                )}
                {semantic?.reason === "invalidated" && (
                  <RemoteFollowUp
                    task={task}
                    grant={grant.id}
                    onCreated={() => client.invalidateQueries()}
                  />
                )}
                {unavailable && (
                  <Hint role="status">
                    {ja
                      ? "相手ノードに接続できません。再読み込みで状態を確認できます。"
                      : "The destination is unavailable. Refresh to reconcile its durable state."}
                  </Hint>
                )}
                {execution?.error && (
                  <Alert>
                    {ja
                      ? "実行の確認が必要です。現在の権限と接続を確認してから再開してください。"
                      : execution.error}
                  </Alert>
                )}
                {!terminal && (
                  <div className="flex min-w-0 flex-wrap gap-2">
                    {live && (!execution || execution.phase === "ADMITTED") && (
                      <Button
                        variant="outline"
                        disabled={busy === grant.id}
                        onClick={() => void act(grant.id, "activate")}
                      >
                        {ja ? "起動を再試行" : "Retry activation"}
                      </Button>
                    )}
                    {execution && execution.phase !== "ADMITTED" && (
                      <>
                        {live &&
                          execution.control === "PAUSED" &&
                          semantic?.reason !== "invalidated" && (
                            <Button
                              variant="outline"
                              disabled={busy === grant.id}
                              onClick={() => void act(grant.id, "resume")}
                            >
                              {ja
                                ? "権限を再確認して再開"
                                : "Recheck authority and resume"}
                            </Button>
                          )}
                        {execution.control === "ACTIVE" && (
                          <Button
                            variant="outline"
                            disabled={busy === grant.id}
                            onClick={() => void act(grant.id, "pause")}
                          >
                            {ja ? "一時停止" : "Pause"}
                          </Button>
                        )}
                        {execution.control !== "CANCELLED" && (
                          <Button
                            variant="destructive"
                            disabled={busy === grant.id}
                            onClick={() => void act(grant.id, "cancel")}
                          >
                            {ja ? "実行を中止" : "Cancel execution"}
                          </Button>
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
          },
        )}
      </div>
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
      className="grid min-w-0 gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
    >
      <Field
        label={
          ja ? "遠隔 Agent への追加指示" : "Instruction for the remote Agent"
        }
      >
        <Textarea
          required
          maxLength={16000}
          value={content}
          disabled={pending !== null}
          onChange={(event) => setContent(event.target.value)}
        />
      </Field>
      {error && <Alert>{error}</Alert>}
      {accepted && (
        <Hint role="status">
          {ja
            ? "指示の受理を確認しました。"
            : "Instruction acceptance confirmed."}
        </Hint>
      )}
      <div className="flex justify-end">
        <Button type="submit" disabled={busy || !content.trim()}>
          {pending
            ? ja
              ? "同じ指示の受理を再確認"
              : "Retry the same instruction"
            : ja
              ? "追加指示を送る"
              : "Send instruction"}
        </Button>
      </div>
    </form>
  );
}

function RemoteHuman({
  task,
  grant,
  request,
}: {
  task: string;
  grant: string;
  request: { id: string; kind: string; prompt: string; response?: unknown };
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [response, setResponse] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  if (request.response != null) return null;
  const answer = async (value: unknown) => {
    setBusy(true);
    setError("");
    try {
      await apiFetch(
        `/api/tasks/${task}/remote-grants/${grant}/human-requests/answer`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ id: request.id, response: value }),
        },
      );
      await client.invalidateQueries({ queryKey: ["remote-executions", task] });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="grid min-w-0 gap-3 border-l-2 border-warning pl-3">
      <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-foreground [overflow-wrap:anywhere]">
        {request.prompt}
      </p>
      {error && <Alert>{error}</Alert>}
      {request.kind === "APPROVAL_REQUIRED" ? (
        <div className="flex min-w-0 flex-wrap gap-2">
          <Button
            disabled={busy}
            onClick={() => void answer({ approved: true })}
          >
            {ja ? "承認" : "Approve"}
          </Button>
          <Button
            disabled={busy}
            variant="outline"
            onClick={() => void answer({ approved: false })}
          >
            {ja ? "拒否" : "Deny"}
          </Button>
        </div>
      ) : (
        <form
          className={formClass}
          onSubmit={(e) => {
            e.preventDefault();
            let value: unknown = response;
            try {
              value = JSON.parse(response);
            } catch {
              /* Plain text is a valid human answer. */
            }
            void answer(value);
          }}
        >
          <Field label={ja ? "応答" : "Response"}>
            <Textarea
              required
              value={response}
              onChange={(e) => setResponse(e.target.value)}
            />
          </Field>
          <div className="flex justify-end">
            <Button type="submit" disabled={busy}>
              {ja ? "応答を送信" : "Send response"}
            </Button>
          </div>
        </form>
      )}
    </div>
  );
}
