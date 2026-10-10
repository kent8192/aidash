import { Button } from "./components/ui/button";
import { useRef, useState } from "react";
import type {
  RemoteGenerationInput,
  RemoteGenerationPrepared,
  EntityRef,
} from "./generated/models";
import type { Submit } from "./forms";
import { ApiError, apiFetch } from "./transport";
import { Field, useI18n } from "./ui";
import { Input } from "./components/ui/input";
import { Textarea } from "./components/ui/textarea";
import { Alert, Check, Disclosure, Facts, Hint } from "./components/patterns";
import type { Entry } from "./types";
import {
  HomeNativeMemoryFields,
  nativeMemoryRequest,
} from "./remote-native-memory";

function reference(value: string): EntityRef {
  const at = value.lastIndexOf("@");
  return { id: value.slice(0, at), version: value.slice(at + 1) };
}

export function RemoteGenerationAssignForm({
  task,
  workspace,
  entries,
  submit,
}: {
  task: string;
  workspace: string;
  entries: Entry[];
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
  const [grantPending, setGrantPending] = useState(false);
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
    <Disclosure
      className="border-t border-border pt-3"
      summary={
        ja ? "別の Node で Agent を生成" : "Generate an agent at another node"
      }
    >
      <Hint>
        {ja
          ? "実行 Node のポリシーを指定して Agent を準備します。必要な承認が完了した後に、実行と Home の記憶の参照を許可できます。"
          : "Prepare an agent under the execution node's policy. After any required approval, authorize its execution and access to Home memory."}
      </Hint>
      <form
        className="grid min-w-0 gap-3"
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
        <fieldset
          disabled={busy || draft !== null}
          className="grid min-w-0 gap-3 sm:grid-cols-2"
        >
          <Field label={ja ? "実行 Node" : "Execution node"}>
            <Input
              name="node"
              required
              placeholder="aidash://node-b"
              className="font-mono"
            />
          </Field>
          <Field
            label={
              ja
                ? "実行 Node の生成ポリシー"
                : "Execution node generation policy"
            }
          >
            <Input name="policy" required className="font-mono" />
          </Field>
          <Field label={ja ? "ポリシーのリビジョン" : "Policy revision"}>
            <Input
              name="revision"
              className="font-mono tabular"
              type="number"
              min={1}
              step={1}
              defaultValue={1}
              required
            />
          </Field>
          <div className="sm:col-span-2">
            <Field label={ja ? "生成を依頼する理由" : "Reason for generation"}>
              <Textarea name="reason" maxLength={4096} rows={2} required />
            </Field>
          </div>
        </fieldset>
        <div className="flex flex-wrap items-center justify-end gap-2">
          {terminalPrepared && (
            <Button
              variant="ghost"
              type="button"
              disabled={busy || grantPending}
              onClick={() => {
                setDraft(null);
                setPrepared(null);
                grant.current = null;
              }}
            >
              {ja ? "新しい依頼を作成" : "Create a new intent"}
            </Button>
          )}
          {draft && !terminalPrepared && (
            <Button
              variant="ghost"
              type="button"
              disabled={busy || grantPending}
              onClick={() => void cancel()}
            >
              {ja ? "準備を中止" : "Cancel preparation"}
            </Button>
          )}
          <Button variant="outline" disabled={busy || !!terminalPrepared}>
            {draft
              ? ja
                ? "同じ準備・承認状態を再確認"
                : "Recheck the same preparation and approval"
              : ja
                ? "Agent を準備"
                : "Prepare agent"}
          </Button>
        </div>
      </form>
      {error && <Alert>{error}</Alert>}
      {prepared && (
        <>
          <Hint role="status">
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
          </Hint>
          <Facts
            items={[
              [
                ja ? "承認対象のリクエスト" : "Request for approval",
                <code className="text-xs">
                  {prepared.node_id} · {prepared.request_id}
                </code>,
              ],
              [
                ja ? "有効期限" : "Expires",
                <time className="font-mono" dateTime={prepared.expires_at}>
                  {new Date(prepared.expires_at).toLocaleString(locale)}
                </time>,
              ],
            ]}
          />
        </>
      )}
      {prepared?.prepared && ["QUEUED", "ACTIVE"].includes(prepared.status) && (
        <form
          className="grid min-w-0 gap-3 border-t border-border pt-3"
          onSubmit={(event) => {
            event.preventDefault();
            const fields = new FormData(event.currentTarget);
            const compactor = String(fields.get("compactor") ?? "").trim();
            setBusy(true);
            void submit(async () => {
              const input = grant.current
                ? JSON.parse(grant.current.binding)
                : {
                    node_id: prepared.node_id,
                    agent: prepared.agent,
                    ttl_seconds: Number(fields.get("lifetime")),
                    semantic: memory
                      ? {
                          mode: "required_home",
                          embedding: reference(String(fields.get("embedding"))),
                          ...(nativeMemoryRequest(fields)
                            ? { native: nativeMemoryRequest(fields) }
                            : {}),
                          ...(compactor
                            ? { compactor: reference(compactor) }
                            : {}),
                        }
                      : { mode: "disabled" },
                  };
              // Keep the initial lifetime and exact body across an uncertain activation.
              const key = JSON.stringify(input);
              if (grant.current?.binding !== key)
                grant.current = { binding: key, id: crypto.randomUUID() };
              const id = grant.current.id;
              setGrantPending(true);
              try {
                await apiFetch(`/api/tasks/${task}/remote-grants`, {
                  method: "POST",
                  headers: { "content-type": "application/json" },
                  body: JSON.stringify({ id, ...input }),
                });
                const result = await apiFetch(
                  `/api/tasks/${task}/remote-grants/${id}/activate`,
                  {
                    method: "POST",
                    headers: { "content-type": "application/json" },
                    body: "{}",
                  },
                );
                grant.current = null;
                setGrantPending(false);
                return result;
              } catch (cause) {
                if (cause instanceof ApiError && cause.responseReceived) {
                  grant.current = null;
                  setGrantPending(false);
                }
                throw cause;
              }
            }).finally(() => setBusy(false));
          }}
        >
          <p className="text-xs text-muted-foreground">
            <code className="text-foreground">
              {prepared.agent.id}@{prepared.agent.version}
            </code>
          </p>
          <fieldset
            disabled={busy || grantPending}
            className="grid min-w-0 gap-3"
          >
            <Field
              label={
                ja
                  ? "実行許可の有効期間（秒）"
                  : "Execution grant lifetime (seconds)"
              }
            >
              <Input
                name="lifetime"
                type="number"
                min={1}
                max={3600}
                defaultValue={600}
                className="font-mono tabular sm:w-40"
                required
              />
            </Field>
            <Check
              checked={memory}
              onChange={(event) => setMemory(event.target.checked)}
            >
              {ja
                ? "Home の記憶を毎回参照"
                : "Require Home memory for each inference"}
            </Check>
            {memory && (
              <>
                <Field
                  label={
                    ja ? "Home の embedding 定義" : "Home embedding definition"
                  }
                >
                  <Input
                    name="embedding"
                    placeholder="embedding-id@1.0.0"
                    pattern=".+@[0-9]+\.[0-9]+\.[0-9]+.*"
                    className="font-mono"
                    required
                  />
                </Field>
                <HomeNativeMemoryFields
                  workspace={workspace}
                  entries={entries}
                  generated
                />
                <Field
                  label={
                    ja
                      ? "任意: 承認済み compactor"
                      : "Optional: approved compactor"
                  }
                >
                  <Input
                    name="compactor"
                    placeholder="compactor-id@1.0.0"
                    pattern=".+@[0-9]+\.[0-9]+\.[0-9]+.*"
                    className="font-mono"
                  />
                </Field>
              </>
            )}
          </fieldset>
          {grantPending && (
            <Hint role="status">
              {ja
                ? "結果が確定するまで、同じ実行許可を再試行します。"
                : "Retry the same execution grant until its outcome is confirmed."}
            </Hint>
          )}
          <div className="flex justify-end">
            <Button disabled={busy}>
              {grantPending
                ? ja
                  ? "同じ実行許可を再試行"
                  : "Retry the same execution grant"
                : ja
                  ? "許可して実行"
                  : "Authorize and execute"}
            </Button>
          </div>
        </form>
      )}
    </Disclosure>
  );
}
