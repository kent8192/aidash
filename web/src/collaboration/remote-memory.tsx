import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/textarea";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type {
  RemoteSemanticStatus,
  RemoteSemanticProvenance,
  RemoteSemanticFailure,
} from "../generated/models";
import { apiFetch } from "../transport";
import { Alert, Facts, Hint, formClass } from "../components/patterns";
import { Field, useI18n } from "../ui";
import type { RunManagement } from "../generated/models";

export function RemoteRunManagement({ id }: { id: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const query = useQuery({
    queryKey: ["run-management", id],
    queryFn: () => apiFetch<RunManagement>(`/api/runs/${id}/management`),
    retry: false,
    refetchInterval: 2000,
  });
  const state = query.isError ? undefined : query.data;
  if (!state) return null;
  const terminal =
    ["COMPLETED", "FAILED", "CANCELLED"].includes(state.phase) ||
    state.control === "CANCELLED";
  const act = (action: "pause" | "cancel") => {
    setBusy(true);
    setError("");
    void apiFetch(`/api/runs/${id}/management`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ action }),
    })
      .then(() => client.invalidateQueries())
      .catch((cause) =>
        setError(cause instanceof Error ? cause.message : String(cause)),
      )
      .finally(() => setBusy(false));
  };
  return (
    <section
      aria-label={ja ? "実行の管理" : "Execution management"}
      className="grid min-w-0 gap-3 border-t border-border pt-4"
    >
      <h3 className="text-[13px] font-semibold text-foreground">
        {ja ? "実行の管理" : "Execution management"}
      </h3>
      {state.semantic_reason && (
        <Hint role="status">{reasons[state.semantic_reason][ja ? 1 : 0]}</Hint>
      )}
      {state.memory_cleanup && (
        <CleanupMessage state={state.memory_cleanup.state} />
      )}
      {!terminal && (
        <div className="flex min-w-0 flex-wrap gap-2">
          {state.control === "ACTIVE" && (
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => act("pause")}
            >
              {ja ? "一時停止" : "Pause"}
            </Button>
          )}
          <Button
            variant="destructive"
            disabled={busy}
            onClick={() => act("cancel")}
          >
            {ja ? "実行を中止" : "Cancel execution"}
          </Button>
        </div>
      )}
      {state.control === "PAUSED" && !terminal && (
        <Hint>
          {ja
            ? "元の Home Node で権限と参照元を確認して再開してください。"
            : "Resume from the original Home node after rechecking authority and sources."}
        </Hint>
      )}
      {error && <Alert>{error}</Alert>}
    </section>
  );
}

function CleanupMessage({ state }: { state: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const labels: Record<string, [string, string]> = {
    pending: ["Scheduled", "予定済み"],
    purged: ["Removed", "除去済み"],
    failed: ["Failed", "失敗"],
  };
  return (
    <Hint role="status">
      {ja ? "受信コピーの清掃" : "Receiver copy cleanup"}:{" "}
      {labels[state]?.[ja ? 1 : 0] ?? state}
    </Hint>
  );
}

const reasons: Record<RemoteSemanticFailure, [string, string]> = {
  configuration: [
    "The approved Home index or provider is unavailable or changed.",
    "承認済みの Home インデックスまたはプロバイダーが利用できないか、変更されています。",
  ],
  authority: [
    "Current authority is required at both nodes. Restore it before an explicit retry.",
    "両 Node の現在の権限が必要です。権限を復元してから明示的に再試行してください。",
  ],
  invalidated: [
    "A consumed source changed. Create a follow-up task with new instructions.",
    "参照済みのソースが変更されました。新しい指示で Follow-up Task を作成してください。",
  ],
  provider_contract: [
    "The provider response did not meet the approved contract.",
    "プロバイダーの応答が承認済みの仕様を満たしていません。",
  ],
  context_budget: [
    "Semantic context does not fit the model's available context budget.",
    "記憶の参照結果がモデルのコンテキスト上限に収まりません。",
  ],
  allowance: [
    "A generated ancestor has insufficient call or token allowance.",
    "生成 Agent の祖先に必要な呼び出し回数またはトークン予算がありません。",
  ],
  unavailable: [
    "Home or its semantic backend is temporarily unavailable.",
    "Home または記憶バックエンドに一時的に接続できません。",
  ],
  retries_exhausted: [
    "Automatic retries are exhausted. An authorized manual retry starts a new bounded cycle.",
    "自動再試行が上限に達しました。権限を確認した手動再試行で新しい試行周期を開始できます。",
  ],
  pending: [
    "The retrieval operation is in progress.",
    "記憶を検索しています。",
  ],
};
const states: Record<string, [string, string]> = {
  disabled: ["Disabled", "無効"],
  pending: ["Pending", "待機中"],
  active: ["Retrieving", "検索中"],
  ready: ["Ready", "取得済み"],
  empty: ["No matches", "該当なし"],
  truncated: ["Bounded results", "一部を省略"],
  waiting: ["Waiting to retry", "再試行待ち"],
  paused: ["Paused", "一時停止"],
  invalidated: ["Sources changed", "参照元が変更済み"],
  cancelled: ["Cancelled", "中止済み"],
  no_space: ["No space in the context budget", "コンテキスト予算に収まらない"],
};

export function RemoteMemoryStatus({
  status,
  provenanceUrl,
}: {
  status: RemoteSemanticStatus;
  provenanceUrl: string;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const language = ja ? 1 : 0;
  const details: (readonly [string, string, boolean])[] = [];
  if (status.retry_count > 0)
    details.push([
      ja ? "自動再試行" : "Automatic retries",
      `${status.retry_count}/5`,
      true,
    ]);
  if (status.retry_at)
    details.push([
      ja ? "次の試行" : "Next attempt",
      new Date(status.retry_at).toLocaleString(locale),
      true,
    ]);
  if (status.result_count != null)
    details.push([
      ja ? "一致件数" : "Matches",
      String(status.result_count),
      true,
    ]);
  return (
    <section
      aria-label={ja ? "Home の記憶" : "Home memory"}
      className="grid min-w-0 gap-2 border-l-2 border-border pl-3"
    >
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
        <h4 className="text-xs font-medium text-muted-foreground">
          {ja ? "Home の記憶" : "Home memory"}
        </h4>
        <p className="text-xs text-foreground">
          {states[status.state]?.[language] ?? status.state}
        </p>
      </div>
      {status.reason && (
        <Hint role="status">{reasons[status.reason][language]}</Hint>
      )}
      {status.body_cleanup && <CleanupMessage state={status.body_cleanup} />}
      {details.length > 0 && <Facts items={details} />}
      {status.state !== "disabled" && !status.reason && (
        <RemoteMemoryProvenance url={provenanceUrl} />
      )}
    </section>
  );
}

export function RemoteMemoryProvenance({ url }: { url: string }) {
  const { locale, t } = useI18n();
  const ja = locale === "ja-JP";
  const [open, setOpen] = useState(false);
  const [inspection, setInspection] = useState(0);
  const query = useQuery({
    queryKey: ["remote-memory-provenance", url, inspection],
    queryFn: () => apiFetch<RemoteSemanticProvenance | null>(url),
    enabled: open,
    retry: false,
    refetchInterval: open ? 5000 : false,
  });
  // Never render cached source identities after a refresh is denied or fails.
  const receipt =
    open && !query.isFetching && !query.isError ? query.data : undefined;
  const label = "text-[11px] font-medium text-faint";
  const row =
    "grid min-w-0 gap-0.5 py-1.5 text-xs text-muted-foreground [overflow-wrap:anywhere]";
  return (
    <div className="grid min-w-0 gap-2">
      <div>
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={() => {
            if (open) setInspection((value) => value + 1);
            setOpen(!open);
          }}
        >
          {open
            ? ja
              ? "参照情報を閉じる"
              : "Hide provenance"
            : ja
              ? "参照情報を確認"
              : "Inspect provenance"}
        </Button>
      </div>
      {open && query.isError && (
        <Hint role="status">
          {ja
            ? "現在の権限で参照情報を確認できません。"
            : "Provenance is unavailable under current authority."}
        </Hint>
      )}
      {open && query.isPending && (
        <Hint role="status">
          {ja ? "権限を確認中…" : "Checking authority…"}
        </Hint>
      )}
      {open && receipt && (
        <div className="grid min-w-0 gap-3">
          <Facts
            items={[
              ["Home Node", receipt.home_node, true],
              [
                ja ? "検索モデル" : "Retrieval model",
                `${receipt.model} · ${receipt.model_version}`,
                true,
              ],
              [
                ja ? "取得日時" : "Retrieved",
                new Date(receipt.retrieved_at).toLocaleString(locale),
                true,
              ],
              [ja ? "実行 Agent" : "Executor", receipt.executor, true],
            ]}
          />
          <div className="grid min-w-0 gap-1.5">
            <h5 className={label}>
              {ja ? "参照元とリビジョン" : "Sources and revisions"}
            </h5>
            <ul className="grid min-w-0 divide-y divide-border border-y border-border">
              {receipt.sources.map((source) => (
                <li key={source.entry_id} className={row}>
                  <span>
                    <code className="font-mono text-foreground">
                      {source.entry_id}
                    </code>{" "}
                    · {source.revision}
                  </span>
                  <code className="font-mono text-faint">
                    {source.content_digest}
                  </code>
                  {source.agent && <p>{source.agent}</p>}
                </li>
              ))}
            </ul>
          </div>
          {(receipt.memory ?? []).map((bank) => (
            <div
              key={JSON.stringify(bank.bank)}
              className="grid min-w-0 gap-1.5"
            >
              <h5 className={label}>
                {bank.bank.participant
                  ? ja
                    ? "Homeの私有記憶"
                    : "Private Home memory"
                  : ja
                    ? "Homeの共有記憶"
                    : "Shared Home memory"}{" "}
                · {states[bank.state]?.[ja ? 1 : 0] ?? bank.state}
              </h5>
              <p className="min-w-0 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]">
                <code>{bank.bank.participant ?? bank.bank.workspace}</code> ·{" "}
                {bank.provider.id}@{bank.provider.version}
              </p>
              <ul className="grid min-w-0 divide-y divide-border border-y border-border">
                {bank.units.map((unit) => (
                  <li key={unit.id} className={row}>
                    <span>
                      <code className="font-mono text-foreground">
                        {unit.id}
                      </code>{" "}
                      · r{unit.revision} · {unit.kind}
                    </span>
                    <p>
                      {unit.verification === "unverified"
                        ? ja
                          ? "未検証"
                          : "Unverified"
                        : unit.verification === "supported"
                          ? ja
                            ? "参照元の裏付けあり"
                            : "Supported by sources"
                          : ja
                            ? "矛盾あり"
                            : "Contradicted"}
                    </p>
                    <code className="font-mono text-faint">
                      {unit.content_digest}
                    </code>
                    <ul className="grid min-w-0 gap-0.5 border-l border-border pl-2">
                      {unit.evidence.map((source, index) => (
                        <li key={index}>
                          <code className="font-mono">
                            {source.kind}: {source.id}
                          </code>{" "}
                          · r{source.revision}
                        </li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {receipt.allowances?.length > 0 && (
            <div className="grid min-w-0 gap-1.5">
              <h5 className={label}>
                {ja
                  ? "この Node の生成予算"
                  : "Generation allowances at this node"}
              </h5>
              <p className="min-w-0 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]">
                {receipt.allowance_node}
              </p>
              {receipt.allowances.map((allowance) => (
                <div
                  key={allowance.request_id}
                  className="grid min-w-0 gap-1.5 border-t border-border pt-2"
                >
                  <code className="min-w-0 font-mono text-xs text-foreground [overflow-wrap:anywhere]">
                    {allowance.request_id}
                  </code>
                  <Facts
                    items={[
                      [
                        t("generationUsedTokens"),
                        `${allowance.used_tokens.toLocaleString()} / ${allowance.token_limit.toLocaleString()}`,
                        true,
                      ],
                      [
                        t("generationEmbeddingCalls"),
                        `${allowance.embedding_calls} / ${allowance.embedding_call_limit}`,
                        true,
                      ],
                      [
                        t("generationCompactionCalls"),
                        `${allowance.compaction_calls} / ${allowance.compaction_call_limit}`,
                        true,
                      ],
                    ]}
                  />
                </div>
              ))}
            </div>
          )}
        </div>
      )}
      {open && receipt === null && (
        <Hint>
          {ja ? "検索結果はまだありません。" : "No retrieval receipt yet."}
        </Hint>
      )}
    </div>
  );
}

export function RemoteFollowUp({
  task,
  grant,
  onCreated,
}: {
  task: string;
  grant: string;
  onCreated: () => Promise<unknown>;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [key, setKey] = useState(() => crypto.randomUUID());
  return (
    <div className="grid min-w-0 gap-3">
      <div>
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={() => setOpen(!open)}
        >
          {ja ? "Follow-up Task を作成" : "Create follow-up task"}
        </Button>
      </div>
      {open && (
        <form
          className={formClass}
          onSubmit={(event) => {
            event.preventDefault();
            setBusy(true);
            setError("");
            void apiFetch(
              `/api/tasks/${task}/remote-grants/${grant}/follow-up`,
              {
                method: "POST",
                headers: { "content-type": "application/json" },
                body: JSON.stringify({
                  id: key,
                  title,
                  description,
                  requirements: {},
                }),
              },
            )
              .then(async () => {
                setOpen(false);
                setTitle("");
                setDescription("");
                setKey(crypto.randomUUID());
                await onCreated();
              })
              .catch((cause) =>
                setError(
                  cause instanceof Error ? cause.message : String(cause),
                ),
              )
              .finally(() => setBusy(false));
          }}
        >
          <Hint>
            {ja
              ? "現在のソースを使うための新しい指示を入力してください。新しい Task を作成した後に、実行する Agent と記憶の参照を設定できます。"
              : "Enter new instructions using current sources. After creating the task, choose its executor and memory access."}
          </Hint>
          <Field label={ja ? "タイトル" : "Title"}>
            <Input
              required
              value={title}
              disabled={busy}
              onChange={(e) => {
                setTitle(e.target.value);
                setKey(crypto.randomUUID());
              }}
            />
          </Field>
          <Field label={ja ? "新しい指示" : "New instructions"}>
            <Textarea
              required
              value={description}
              disabled={busy}
              onChange={(e) => {
                setDescription(e.target.value);
                setKey(crypto.randomUUID());
              }}
            />
          </Field>
          {error && <Alert>{error}</Alert>}
          <div className="flex justify-end">
            <Button type="submit" disabled={busy}>
              {ja ? "作成" : "Create"}
            </Button>
          </div>
        </form>
      )}
    </div>
  );
}
