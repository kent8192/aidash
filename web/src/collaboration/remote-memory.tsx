import { Button } from "../components/ui/button";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type {
  RemoteSemanticStatus,
  RemoteSemanticProvenance,
  RemoteSemanticFailure,
} from "../generated/models";
import { apiFetch } from "../transport";
import { useI18n } from "../ui";
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
    <section aria-label={ja ? "実行の管理" : "Execution management"}>
      {state.semantic_reason && (
        <p role="status">{reasons[state.semantic_reason][ja ? 1 : 0]}</p>
      )}
      {!terminal && (
        <div className="button-row">
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
            variant="outline"
            disabled={busy}
            onClick={() => act("cancel")}
          >
            {ja ? "実行を中止" : "Cancel execution"}
          </Button>
        </div>
      )}
      {state.control === "PAUSED" && !terminal && (
        <p>
          {ja
            ? "元の Home Node で権限と参照元を確認して再開してください。"
            : "Resume from the original Home node after rechecking authority and sources."}
        </p>
      )}
      {error && <p role="alert">{error}</p>}
    </section>
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
  return (
    <section aria-label={ja ? "Home の記憶" : "Home memory"}>
      <h4>{ja ? "Home の記憶" : "Home memory"}</h4>
      <p>{states[status.state]?.[language] ?? status.state}</p>
      {status.reason && <p role="status">{reasons[status.reason][language]}</p>}
      {status.retry_count > 0 && (
        <p>
          {ja ? "自動再試行" : "Automatic retries"}: {status.retry_count}/5
        </p>
      )}
      {status.retry_at && (
        <p>
          {ja ? "次の試行" : "Next attempt"}:{" "}
          {new Date(status.retry_at).toLocaleString(locale)}
        </p>
      )}
      {status.result_count != null && (
        <p>
          {ja ? "一致件数" : "Matches"}: {status.result_count}
        </p>
      )}
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
  return (
    <div>
      <Button
        variant="outline"
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
      {open && query.isError && (
        <p role="status">
          {ja
            ? "現在の権限で参照情報を確認できません。"
            : "Provenance is unavailable under current authority."}
        </p>
      )}
      {open && query.isPending && (
        <p role="status">{ja ? "権限を確認中…" : "Checking authority…"}</p>
      )}
      {open && receipt && (
        <dl>
          <dt>Home Node</dt>
          <dd>{receipt.home_node}</dd>
          <dt>{ja ? "検索モデル" : "Retrieval model"}</dt>
          <dd>
            {receipt.model} · {receipt.model_version}
          </dd>
          <dt>{ja ? "取得日時" : "Retrieved"}</dt>
          <dd>{new Date(receipt.retrieved_at).toLocaleString(locale)}</dd>
          <dt>{ja ? "実行 Agent" : "Executor"}</dt>
          <dd>
            <code>{receipt.executor}</code>
          </dd>
          <dt>{ja ? "参照元とリビジョン" : "Sources and revisions"}</dt>
          <dd>
            <ul>
              {receipt.sources.map((source) => (
                <li key={source.entry_id}>
                  <code>{source.entry_id}</code> · {source.revision}
                  <br />
                  <code>{source.content_digest}</code>
                  {source.agent && <p>{source.agent}</p>}
                </li>
              ))}
            </ul>
          </dd>
          {receipt.allowances?.length > 0 && (
            <>
              <dt>
                {ja
                  ? "この Node の生成予算"
                  : "Generation allowances at this node"}
              </dt>
              <dd>{receipt.allowance_node}</dd>
              {receipt.allowances.map((allowance) => (
                <dd key={allowance.request_id}>
                  <code>{allowance.request_id}</code>
                  <dl>
                    <dt>{t("generationUsedTokens")}</dt>
                    <dd>
                      {allowance.used_tokens.toLocaleString()} /{" "}
                      {allowance.token_limit.toLocaleString()}
                    </dd>
                    <dt>{t("generationEmbeddingCalls")}</dt>
                    <dd>
                      {allowance.embedding_calls} /{" "}
                      {allowance.embedding_call_limit}
                    </dd>
                    <dt>{t("generationCompactionCalls")}</dt>
                    <dd>
                      {allowance.compaction_calls} /{" "}
                      {allowance.compaction_call_limit}
                    </dd>
                  </dl>
                </dd>
              ))}
            </>
          )}
        </dl>
      )}
      {open && receipt === null && (
        <p>{ja ? "検索結果はまだありません。" : "No retrieval receipt yet."}</p>
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
    <div>
      <Button variant="outline" type="button" onClick={() => setOpen(!open)}>
        {ja ? "Follow-up Task を作成" : "Create follow-up task"}
      </Button>
      {open && (
        <form
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
          <p>
            {ja
              ? "現在のソースを使うための新しい指示を入力してください。新しい Task を作成した後に、実行する Agent と記憶の参照を設定できます。"
              : "Enter new instructions using current sources. After creating the task, choose its executor and memory access."}
          </p>
          <label>
            {ja ? "タイトル" : "Title"}
            <input
              required
              value={title}
              disabled={busy}
              onChange={(e) => {
                setTitle(e.target.value);
                setKey(crypto.randomUUID());
              }}
            />
          </label>
          <label>
            {ja ? "新しい指示" : "New instructions"}
            <textarea
              required
              value={description}
              disabled={busy}
              onChange={(e) => {
                setDescription(e.target.value);
                setKey(crypto.randomUUID());
              }}
            />
          </label>
          {error && <p role="alert">{error}</p>}
          <Button variant="outline" disabled={busy}>
            {ja ? "作成" : "Create"}
          </Button>
        </form>
      )}
    </div>
  );
}
