import { useState } from "react";
import type { Entry } from "./types";
import { Field, useEntityLabel, useI18n } from "./ui";

const reference = (value: string) => {
  const at = value.lastIndexOf("@");
  if (at < 1 || at === value.length - 1)
    throw new Error("Select an exact Registry version");
  return { id: value.slice(0, at), version: value.slice(at + 1) };
};
const bounds = {
  max_unit_bytes: 8192,
  max_input_bytes: 16384,
  max_units: 64,
  max_candidates: 64,
  max_entities: 32,
  max_evidence: 32,
  max_links: 32,
  max_graph_hops: 4,
  max_graph_visits: 1024,
  max_results: 16,
  max_context_tokens: 16384,
  max_model_calls: 8,
  max_model_tokens: 65536,
  max_cost_micros: 1000000,
  max_retries: 3,
  max_call_seconds: 60,
};
const retention = {
  candidate_days: 7,
  history_days: 30,
  history_versions: 32,
  model_result_days: 7,
  backup_days: 7,
  purge_after_seconds: 60,
  purge_batch: 32,
  max_unit_records: 4096,
  max_model_operations: 8192,
};
const boundLabels: Record<string, [string, string]> = {
  max_unit_bytes: ["Unit bytes", "Unitのバイト数"],
  max_input_bytes: ["Input bytes", "入力バイト数"],
  max_units: ["Admitted units", "採用済みUnit数"],
  max_candidates: ["Candidates per operation", "1回の候補数"],
  max_entities: ["Entities per unit", "Unitのエンティティ数"],
  max_evidence: ["Evidence items per unit", "Unitの根拠数"],
  max_links: ["Links per unit", "Unitのリンク数"],
  max_graph_hops: ["Graph hops", "グラフの深さ"],
  max_graph_visits: ["Graph visits", "グラフの参照数"],
  max_results: ["Recall results", "Recallの結果数"],
  max_context_tokens: ["Context tokens", "コンテキストのトークン数"],
  max_model_calls: ["Model calls per operation", "1回のモデル呼び出し数"],
  max_model_tokens: ["Model tokens per operation", "1回のモデルトークン数"],
  max_cost_micros: [
    "Cost limit (microcurrency)",
    "費用上限（通貨の百万分の一）",
  ],
  max_retries: ["Attempts", "試行回数"],
  max_call_seconds: ["Call timeout (seconds)", "呼び出しの制限時間（秒）"],
  candidate_days: ["Candidate retention (days)", "候補の保持日数"],
  history_days: ["Revision retention (days)", "履歴の保持日数"],
  history_versions: ["Retained revisions per unit", "Unitの保持revision数"],
  model_result_days: ["Model result retention (days)", "モデル結果の保持日数"],
  backup_days: ["Backup retention (days)", "バックアップの保持日数"],
  purge_after_seconds: [
    "Physical cleanup delay (seconds)",
    "物理消去までの秒数",
  ],
  purge_batch: ["Cleanup batch", "消去のバッチ件数"],
  max_unit_records: [
    "Unit and deletion identity records",
    "Unit・削除識別子の保存件数",
  ],
  max_model_operations: ["Operation and job records", "操作・処理の保存件数"],
};
export function memoryConfiguration(
  kind: string,
  form: FormData,
): Record<string, unknown> | undefined {
  const text = (key: string) => String(form.get(key) ?? "");
  if (
    ["memory", "source"].includes(kind) &&
    text("context_adapter") !== "native_memory"
  )
    return undefined;
  const number = (key: string) => {
    const value = Number(text(key));
    if (!Number.isSafeInteger(value) || value < 0)
      throw new Error("Enter a finite nonnegative integer");
    return value;
  };
  if (kind === "reranker")
    return text("memory_reranker_kind") === "model"
      ? { provider: "model", model: reference(text("memory_role_model")) }
      : { provider: "rrf" };
  if (kind === "tokenizer") return { provider: "utf8_upper_bound" };
  if (kind === "source")
    return {
      scope: text("memory_scope"),
      memory: reference(text("memory_provider")),
      max_tokens: number("source_tokens"),
    };
  if (kind !== "memory") return undefined;
  const roles = [
    "extraction",
    "derivation",
    "reflection",
    "embedding",
    "reranker",
    "tokenizer",
  ];
  return {
    engine: "hindsight_rust",
    policy: {
      ...Object.fromEntries(
        roles.map((role) => [role, reference(text(`memory_role_${role}`))]),
      ),
      learn_from_runs: form.has("memory_learning"),
      maintain_observations: form.has("memory_observations"),
      refresh_mental_models: form.has("memory_refresh"),
      semantic_link_min_similarity_millionths: number(
        "semantic_link_min_similarity_millionths",
      ),
      bounds: Object.fromEntries(
        Object.keys(bounds).map((key) => [key, number(key)]),
      ),
      retention: {
        ...Object.fromEntries(
          Object.keys(retention).map((key) => [key, number(key)]),
        ),
        unit_max_age_days: text("unit_max_age_days")
          ? number("unit_max_age_days")
          : null,
      },
      prices: Object.fromEntries(
        roles
          .filter((role) => role !== "tokenizer")
          .map((role) => [
            role,
            {
              input_per_million: number(`price_${role}_input`),
              output_per_million: number(`price_${role}_output`),
            },
          ]),
      ),
    },
  };
}
function ReferenceField({
  entries,
  kind,
  name,
  label,
  optional = false,
}: {
  entries: Entry[];
  kind: string;
  name: string;
  label: string;
  optional?: boolean;
}) {
  const entityLabel = useEntityLabel(entries);
  const { t } = useI18n();
  return (
    <Field label={label}>
      <select name={name} required={!optional} defaultValue="">
        <option value="">{t("choose")}</option>
        {entries
          .filter((entry) => entry.kind === kind)
          .map((entry) => (
            <option
              key={`${entry.id}@${entry.version}`}
              value={`${entry.id}@${entry.version}`}
            >
              {entityLabel(entry)}
            </option>
          ))}
      </select>
    </Field>
  );
}
export function AgentMemoryFields({
  entries,
  initial,
}: {
  entries: Entry[];
  initial?: Record<string, unknown>;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const value = initial?.memory as
    | { id: string; version: string }
    | null
    | undefined;
  const entityLabel = useEntityLabel(entries);
  return (
    <fieldset>
      <legend>{ja ? "メモリ" : "Memory"}</legend>
      <Field label={ja ? "個人メモリのポリシー" : "Private memory policy"}>
        <select
          name="memory_provider"
          defaultValue={value ? `${value.id}@${value.version}` : ""}
        >
          <option value="">{ja ? "無効" : "Disabled"}</option>
          {entries
            .filter((entry) => entry.kind === "memory")
            .map((entry) => (
              <option
                key={`${entry.id}@${entry.version}`}
                value={`${entry.id}@${entry.version}`}
              >
                {entityLabel(entry)}
              </option>
            ))}
        </select>
      </Field>
      <label>
        <input
          type="checkbox"
          name="allow_memory_write"
          defaultChecked={initial?.allow_memory_write === true}
        />
        {ja
          ? "このAgentによるUnitの変更を許可"
          : "Allow unit changes by this Agent"}
      </label>
      <Field label={ja ? "追加の検索元" : "Additional recall sources"}>
        <select
          multiple
          name="memory_sources"
          defaultValue={
            Array.isArray(initial?.sources)
              ? initial.sources.map(
                  (ref: { id: string; version: string }) =>
                    `${ref.id}@${ref.version}`,
                )
              : []
          }
        >
          {entries
            .filter((entry) => entry.kind === "source")
            .map((entry) => (
              <option
                key={`${entry.id}@${entry.version}`}
                value={`${entry.id}@${entry.version}`}
              >
                {entityLabel(entry)}
              </option>
            ))}
        </select>
      </Field>
      <p className="muted">
        {ja
          ? "本文はWorkspaceに保持されます。選択した定義のバージョンを固定します。"
          : "Bodies belong to the Workspace. Selected definition versions are pinned."}
      </p>
    </fieldset>
  );
}
export function MemoryRegistryFields({
  kind,
  entries,
}: {
  kind: string;
  entries: Entry[];
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [reranker, setReranker] = useState("rrf");
  const roles: [string, string, string][] = [
    ["extraction", "Extraction", "抽出"],
    ["derivation", "Derivation", "派生"],
    ["reflection", "Reflection", "考察"],
    ["embedding", "Embedding", "埋め込み"],
    ["reranker", "Reranker", "再順位付け"],
    ["tokenizer", "Tokenizer", "トークン計算"],
  ];
  if (kind === "tokenizer")
    return (
      <p>
        {ja
          ? "UTF-8バイト数による保守的な上限。根拠を含む全体を計算します。"
          : "Conservative UTF-8 byte upper bound, counting the complete provenance envelope."}
      </p>
    );
  if (kind === "reranker")
    return (
      <>
        <Field label={ja ? "順位付け方法" : "Ranking method"}>
          <select
            name="memory_reranker_kind"
            value={reranker}
            onChange={(e) => setReranker(e.target.value)}
          >
            <option value="rrf">RRF</option>
            <option value="model">{ja ? "モデル" : "Model"}</option>
          </select>
        </Field>
        {reranker === "model" && (
          <ReferenceField
            entries={entries}
            kind="model"
            name="memory_role_model"
            label={ja ? "モデル" : "Model"}
          />
        )}
      </>
    );
  if (kind === "source")
    return (
      <>
        <ReferenceField
          entries={entries}
          kind="memory"
          name="memory_provider"
          label={ja ? "メモリポリシー" : "Memory policy"}
        />
        <Field label={ja ? "検索する範囲" : "Recall scope"}>
          <select name="memory_scope">
            <option value="workspace">
              {ja ? "共有Workspace" : "Shared Workspace"}
            </option>
            <option value="participant">
              {ja ? "論理Agentの個人メモリ" : "Logical Agent private memory"}
            </option>
          </select>
        </Field>
        <Field
          label={
            ja ? "根拠を含むトークン上限" : "Token cap including provenance"
          }
        >
          <input
            name="source_tokens"
            type="number"
            min={1}
            max={2147483647}
            defaultValue={8192}
            required
          />
        </Field>
      </>
    );
  return (
    <>
      <p>
        {ja
          ? "RustのHindsight。モデル・料金・上限を固定します。"
          : "Native Rust Hindsight with pinned roles, prices and finite limits."}
      </p>
      {roles.map(([role, en, jp]) => (
        <ReferenceField
          key={role}
          entries={entries}
          kind={
            ["embedding", "reranker", "tokenizer"].includes(role)
              ? role
              : "model"
          }
          name={`memory_role_${role}`}
          label={ja ? jp : en}
        />
      ))}
      <label>
        <input type="checkbox" name="memory_learning" />
        {ja
          ? "完了Runからレビュー待ち候補を作成"
          : "Propose review candidates from completed Runs"}
      </label>
      <label>
        <input type="checkbox" name="memory_observations" defaultChecked />
        {ja
          ? "採用済みUnitからObservationを更新"
          : "Maintain observations from admitted units"}
      </label>
      <label>
        <input type="checkbox" name="memory_refresh" defaultChecked />
        {ja
          ? "選択された定期的な問いを更新"
          : "Refresh selected recurring questions"}
      </label>
      <fieldset>
        <legend>{ja ? "意味的な関連づけ" : "Semantic links"}</legend>
        <Field
          label={
            ja
              ? "コサイン類似度の下限（百万分率）"
              : "Minimum cosine similarity (millionths)"
          }
        >
          <input
            name="semantic_link_min_similarity_millionths"
            type="number"
            min={1}
            max={1000000}
            defaultValue={700000}
            required
          />
        </Field>
      </fieldset>
      <fieldset>
        <legend>
          {ja
            ? "固定した料金（百万トークンあたりの通貨の百万分の一）"
            : "Pinned prices (microcurrency per million tokens)"}
        </legend>
        {roles
          .filter(([role]) => role !== "tokenizer")
          .map(([role, en, jp]) => (
            <div key={role}>
              {["input", "output"].map((direction) => (
                <Field
                  key={direction}
                  label={`${ja ? jp : en} · ${ja ? (direction === "input" ? "入力" : "出力") : direction}`}
                >
                  <input
                    name={`price_${role}_${direction}`}
                    type="number"
                    min={0}
                    max={Number.MAX_SAFE_INTEGER}
                    required
                  />
                </Field>
              ))}
            </div>
          ))}
      </fieldset>
      <details>
        <summary>
          {ja ? "処理・保存の上限" : "Operation and storage limits"}
        </summary>
        {Object.entries({ ...bounds, ...retention }).map(([key, value]) => (
          <Field key={key} label={boundLabels[key][ja ? 1 : 0]}>
            <input
              name={key}
              type="number"
              min={1}
              max={key.endsWith("days") ? 3650 : 2147483647}
              defaultValue={value}
              required
            />
          </Field>
        ))}
        <Field
          label={
            ja
              ? "採用済みUnitの保持日数（空欄なら無期限）"
              : "Admitted unit retention days (blank keeps indefinitely)"
          }
        >
          <input name="unit_max_age_days" type="number" min={1} max={3650} />
        </Field>
      </details>
    </>
  );
}
