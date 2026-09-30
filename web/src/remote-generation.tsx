import { useRef, useState } from "react";
import type {
  RemoteGenerationInput,
  RemoteGenerationPrepared,
  EntityRef,
} from "./generated/models";
import type { Submit } from "./forms";
import { apiFetch } from "./transport";
import { Field, useI18n } from "./ui";

function reference(value: string): EntityRef {
  const at = value.lastIndexOf("@");
  return { id: value.slice(0, at), version: value.slice(at + 1) };
}

export function RemoteGenerationAssignForm({
  task,
  submit,
}: {
  task: string;
  submit: Submit;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [draft, setDraft] = useState<RemoteGenerationInput | null>(null);
  const [prepared, setPrepared] = useState<RemoteGenerationPrepared | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const grant = useRef<{ binding: string; id: string } | null>(null);
  const [memory, setMemory] = useState(true);
  const terminalPrepared =
    prepared &&
    !["PENDING_APPROVAL", "QUEUED", "ACTIVE"].includes(prepared.status);
  const prepare = async (input: RemoteGenerationInput) => {
    setDraft(input);
    setBusy(true);
    setError("");
    try {
      setPrepared(
        await apiFetch<RemoteGenerationPrepared>(
          `/api/tasks/${task}/remote-generation`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(input),
          },
        ),
      );
    } catch (cause) {
      setPrepared(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  const cancel = async () => {
    if (!draft) return;
    setBusy(true);
    setError("");
    try {
      await apiFetch(
        `/api/tasks/${task}/remote-generation/${draft.id}/cancel`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: "{}",
        },
      );
      setDraft(null);
      setPrepared(null);
      grant.current = null;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <details className="detail-section">
      <summary>
        {ja ? "別の Node で Agent を生成" : "Generate an agent at another node"}
      </summary>
      <p>
        {ja
          ? "実行 Node のポリシーを指定して Agent を準備します。必要な承認が完了した後に、実行と Home の記憶の参照を許可できます。"
          : "Prepare an agent under the execution node's policy. After any required approval, authorize its execution and access to Home memory."}
      </p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          void prepare(
            draft ?? {
              id: crypto.randomUUID(),
              node_id: String(data.get("node")),
              policy_id: String(data.get("policy")),
              policy_revision: Number(data.get("revision")),
              ttl_seconds: 3600,
              reason: String(data.get("reason")),
            },
          );
        }}
      >
        <fieldset disabled={busy || draft !== null}>
          <Field label={ja ? "実行 Node" : "Execution node"}>
            <input name="node" required placeholder="aidash://node-b" />
          </Field>
          <Field
            label={
              ja
                ? "実行 Node の生成ポリシー"
                : "Execution node generation policy"
            }
          >
            <input name="policy" required />
          </Field>
          <Field label={ja ? "ポリシーのリビジョン" : "Policy revision"}>
            <input
              name="revision"
              type="number"
              min={1}
              step={1}
              defaultValue={1}
              required
            />
          </Field>
          <Field label={ja ? "生成を依頼する理由" : "Reason for generation"}>
            <textarea name="reason" maxLength={4096} required />
          </Field>
        </fieldset>
        <button disabled={busy || !!terminalPrepared}>
          {draft
            ? ja
              ? "同じ準備・承認状態を再確認"
              : "Recheck the same preparation and approval"
            : ja
              ? "Agent を準備"
              : "Prepare agent"}
        </button>
      </form>
      {error && <p role="alert">{error}</p>}
      {terminalPrepared && (
        <button
          type="button"
          disabled={busy}
          onClick={() => {
            setDraft(null);
            setPrepared(null);
            grant.current = null;
          }}
        >
          {ja ? "新しい依頼を作成" : "Create a new intent"}
        </button>
      )}
      {draft && !terminalPrepared && (
        <button type="button" disabled={busy} onClick={() => void cancel()}>
          {ja ? "準備を中止" : "Cancel preparation"}
        </button>
      )}
      {prepared && (
        <>
          <p role="status">
            {terminalPrepared
              ? ja
                ? "準備は終了しました。新しい依頼を作成してください。"
                : "Preparation has ended. Create a new intent."
              : prepared.prepared
                ? ja
                  ? "準備が完了しました。実行許可を設定できます。"
                  : "Prepared. Configure its execution grant."
                : ja
                  ? "実行 Node で必要な承認を完了してから再確認してください。"
                  : "Complete the required approval at the execution node, then recheck."}
          </p>
          <dl>
            <dt>{ja ? "承認対象のリクエスト" : "Request for approval"}</dt>
            <dd>
              <code>
                {prepared.node_id} · {prepared.request_id}
              </code>
            </dd>
            <dt>{ja ? "有効期限" : "Expires"}</dt>
            <dd>{new Date(prepared.expires_at).toLocaleString(locale)}</dd>
          </dl>
        </>
      )}
      {prepared?.prepared && ["QUEUED", "ACTIVE"].includes(prepared.status) && (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const fields = new FormData(event.currentTarget);
            const compactor = String(fields.get("compactor") ?? "").trim();
            setBusy(true);
            void submit(async () => {
              const input = {
                node_id: prepared.node_id,
                agent: prepared.agent,
                ttl_seconds: Number(fields.get("lifetime")),
                semantic: memory
                  ? {
                      mode: "required_home",
                      embedding: reference(String(fields.get("embedding"))),
                      ...(compactor ? { compactor: reference(compactor) } : {}),
                    }
                  : { mode: "disabled" },
              };
              // Keep the initial lifetime and exact body across an uncertain activation.
              const key = JSON.stringify(input);
              if (grant.current?.binding !== key)
                grant.current = { binding: key, id: crypto.randomUUID() };
              const id = grant.current.id;
              await apiFetch(`/api/tasks/${task}/remote-grants`, {
                method: "POST",
                headers: { "content-type": "application/json" },
                body: JSON.stringify({ id, ...input }),
              });
              return apiFetch(
                `/api/tasks/${task}/remote-grants/${id}/activate`,
                {
                  method: "POST",
                  headers: { "content-type": "application/json" },
                  body: "{}",
                },
              );
            }).finally(() => setBusy(false));
          }}
        >
          <p>
            <code>
              {prepared.agent.id}@{prepared.agent.version}
            </code>
          </p>
          <Field
            label={
              ja
                ? "実行許可の有効期間（秒）"
                : "Execution grant lifetime (seconds)"
            }
          >
            <input
              name="lifetime"
              type="number"
              min={1}
              max={3600}
              defaultValue={600}
              required
            />
          </Field>
          <label>
            <input
              type="checkbox"
              checked={memory}
              onChange={(event) => setMemory(event.target.checked)}
            />
            {ja
              ? "Home の記憶を毎回参照"
              : "Require Home memory for each inference"}
          </label>
          {memory && (
            <>
              <Field
                label={
                  ja ? "Home の embedding 定義" : "Home embedding definition"
                }
              >
                <input
                  name="embedding"
                  placeholder="embedding-id@1.0.0"
                  pattern=".+@[0-9]+\.[0-9]+\.[0-9]+.*"
                  required
                />
              </Field>
              <Field
                label={
                  ja
                    ? "任意: 承認済み compactor"
                    : "Optional: approved compactor"
                }
              >
                <input
                  name="compactor"
                  placeholder="compactor-id@1.0.0"
                  pattern=".+@[0-9]+\.[0-9]+\.[0-9]+.*"
                />
              </Field>
            </>
          )}
          <button className="primary" disabled={busy}>
            {ja ? "許可して実行" : "Authorize and execute"}
          </button>
        </form>
      )}
    </details>
  );
}
