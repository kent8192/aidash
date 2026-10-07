import { useI18n } from "./ui";
import { useRecordLabels } from "./record-view";
import type { Content, Evidence, Unit } from "./memory";
import "./memory.css";

const names: Record<string, [string, string]> = {
  recall: ["Recall", "検索"],
  reflection: ["Reflection", "考察"],
  usage: ["Model usage", "モデル使用量"],
  causes: ["Causes", "因果関係"],
  enables: ["Enables", "実現する関係"],
  prevents: ["Prevents", "防ぐ関係"],
  kind: ["Kind", "種類"],
  learning: ["Learning type", "学習の種類"],
  verification: ["Verification", "検証状態"],
  publication: ["Publication", "公開記録"],
  message: ["Message", "メッセージ"],
  artifact: ["Artifact", "成果物"],
  run: ["Run", "Run"],
  calls: ["Model calls", "モデル呼び出し数"],
  disclosure_current: ["Disclosure current", "公開権限が有効"],
  dependent_units: ["Dependent memories", "依存する記憶"],
  purge_after: ["Cleanup after", "消去予定日時"],
  backup_until: ["Backup retention until", "バックアップ保持期限"],
  next_attempt: ["Next attempt", "次回の試行"],
  operation_id: ["Operation", "操作 ID"],
  actor: ["Actor", "操作した主体"],
  world: ["World knowledge", "世界の知識"],
  experience: ["Experience", "経験"],
  observation: ["Observation", "観察"],
  mental_model: ["Mental model", "メンタルモデル"],
  fact: ["Fact", "事実"],
  preference: ["Preference", "好み"],
  procedure: ["Procedure", "手順"],
  failure: ["Failure", "失敗"],
  unverified: ["Unverified", "未検証"],
  supported: ["Supported", "裏付けあり"],
  contradicted: ["Contradicted", "矛盾あり"],
  pending: ["Pending", "確認待ち"],
  admitted: ["Admitted", "採用済み"],
  rejected: ["Rejected", "却下"],
  invalidated: ["Invalidated", "無効"],
  ready: ["Ready", "準備完了"],
  empty: ["No matching memories", "該当する記憶なし"],
  disabled: ["Disabled", "無効"],
  no_space: ["Context budget exhausted", "コンテキスト上限に到達"],
  content: ["Content", "内容"],
  text: ["Text", "本文"],
  question: ["Question", "問い"],
  automatic_refresh: ["Automatic refresh", "自動更新"],
  id: ["ID", "ID"],
  unit_id: ["Memory unit", "記憶 Unit"],
  revision: ["Revision", "リビジョン"],
  state: ["State", "状態"],
  status: ["Status", "状態"],
  provider: ["Provider", "プロバイダ"],
  bank: ["Memory bank", "記憶の所有者"],
  home: ["Home", "Home"],
  tenant: ["Tenant", "テナント"],
  workspace: ["Workspace", "Workspace"],
  participant: ["Logical Agent", "論理 Agent"],
  learned_at: ["Learned", "記憶した日時"],
  updated_at: ["Updated", "更新日時"],
  created_at: ["Created", "作成日時"],
  deleted: ["Deleted", "削除済み"],
  stale: ["Stale", "更新が必要"],
  evidence: ["Evidence", "根拠"],
  source: ["Source", "出典"],
  result: ["Result", "結果"],
  value: ["Details", "詳細"],
  items: ["Records", "記録"],
  units: ["Memories", "記憶"],
  dependent_runs: ["Affected Runs", "影響する Run"],
  model_operations: ["Model operations", "モデル処理数"],
  pending_candidates: ["Pending candidates", "確認待ちの学習候補"],
  purge_jobs: ["Cleanup jobs", "消去処理"],
  attempts: ["Attempts", "試行回数"],
  last_error: ["Last error", "最後のエラー"],
  next_attempt_at: ["Next attempt", "次回の試行"],
  tokens: ["Tokens", "トークン数"],
  cost_micros: ["Charged cost (µUSD)", "計上コスト（µUSD）"],
};
function useMemoryLabels() {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  return {
    locale,
    text: (en: string, jp: string) => (ja ? jp : en),
    label: (key: string) =>
      names[key]?.[ja ? 1 : 0] ??
      key.replaceAll("_", " ").replace(/^./, (first) => first.toUpperCase()),
  };
}
const record = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const unitAnchor = (id: string) => `memory-unit-${id}`;

export function MemoryOverview({ units }: { units: Unit[] }) {
  const { text } = useMemoryLabels();
  const metrics = [
    [text("Memories", "記憶"), units.length],
    [
      text("Supported", "裏付けあり"),
      units.filter((u) => u.content.verification === "supported").length,
    ],
    [
      text("To verify", "未検証"),
      units.filter((u) => u.content.verification === "unverified").length,
    ],
    [
      text("Needs attention", "確認が必要"),
      units.filter((u) => u.stale || u.content.verification === "contradicted")
        .length,
    ],
  ];
  return (
    <dl
      className="memory-overview"
      aria-label={text("Memory overview", "記憶の概要")}
    >
      {metrics.map(([name, count]) => (
        <div key={name}>
          <dt>{name}</dt>
          <dd>{count}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Only link to a source whose bank and observed revision are already disclosed. */
function EvidenceView({ value, units }: { value: Evidence; units: Unit[] }) {
  const { text, label } = useMemoryLabels();
  const labels = useRecordLabels();
  const source =
    value.kind === "unit" &&
    units.find(
      (unit) =>
        unit.id === value.id &&
        unit.revision === value.revision &&
        value.bank &&
        unit.bank.home === value.bank.home &&
        unit.bank.tenant === value.bank.tenant &&
        unit.bank.workspace === value.bank.workspace &&
        unit.bank.participant === value.bank.participant,
    );
  const title = source
    ? source.content.text
    : (labels.get(value.id) ?? value.id);
  return (
    <div className="memory-evidence-node">
      <span className="memory-tag">
        {value.kind === "unit" ? text("Memory", "記憶") : label(value.kind)}
      </span>
      <span className="memory-evidence-title">
        {source ? (
          <button
            type="button"
            onClick={() => {
              const target = document.getElementById(unitAnchor(source.id));
              target?.scrollIntoView({ block: "center" });
              target?.focus({ preventScroll: true });
            }}
          >
            {title}
          </button>
        ) : (
          <span>{title}</span>
        )}
        <small>
          {text("Revision", "リビジョン")} {value.revision}
        </small>
      </span>
      {value.bank && (
        <small>
          {value.bank.participant
            ? text("Private", "プライベート")
            : text("Shared", "共有")}
        </small>
      )}
      {value.digest && (
        <small className="memory-digest">
          {text("Digest", "ダイジェスト")}: {value.digest}
        </small>
      )}
    </div>
  );
}

export function MemoryContent({
  content,
  units = [],
  unit,
}: {
  content: Partial<Content>;
  units?: Unit[];
  unit?: Unit;
}) {
  const { locale, label, text } = useMemoryLabels();
  return (
    <div className="memory-content">
      <div className="memory-tags">
        {[
          content.kind,
          content.learning,
          content.verification,
          unit?.stale ? "stale" : null,
          unit?.deleted ? "deleted" : null,
        ]
          .filter((value): value is string => !!value)
          .map((value) => (
            <span className={`memory-tag ${value}`} key={value}>
              {label(value)}
            </span>
          ))}
      </div>
      {content.mental_model && (
        <div className="memory-question">
          <strong>{content.mental_model.question}</strong>
          <small>
            {content.mental_model.automatic_refresh
              ? text("Automatic refresh", "自動更新")
              : text("Manual refresh", "手動更新")}
          </small>
        </div>
      )}
      <p className="memory-body">{content.text}</p>
      {!!content.entities?.length && (
        <ul
          className="memory-entities"
          aria-label={text("Entities", "エンティティ")}
        >
          {content.entities.map((entity, index) => (
            <li key={index}>
              <strong>{entity.name}</strong>
              <span>{entity.category}</span>
              {!!entity.aliases?.length && (
                <small>{entity.aliases.join(" · ")}</small>
              )}
            </li>
          ))}
        </ul>
      )}
      {unit && (
        <div className="memory-dates">
          <span>
            {label("revision")} {unit.revision}
          </span>
          <span>
            {label("updated_at")}{" "}
            <time dateTime={unit.updated_at}>
              {new Date(unit.updated_at).toLocaleString(locale)}
            </time>
          </span>
        </div>
      )}
      <details className="memory-sources">
        <summary>
          {text("Sources, entities and time", "出典・エンティティ・時刻")} ·{" "}
          {content.evidence?.length ?? 0}
        </summary>
        {content.evidence?.length ? (
          <div
            className="memory-provenance"
            aria-label={text("Evidence connections", "根拠のつながり")}
          >
            <ul>
              {content.evidence.map((proof, index) => (
                <li key={index}>
                  <EvidenceView value={proof} units={units} />
                </li>
              ))}
            </ul>
            <div className="memory-provenance-target">
              <span aria-hidden="true">↓</span>
              {text("Cited by this memory", "この記憶が引用")}
            </div>
          </div>
        ) : (
          <p className="muted">
            {text("No cited evidence", "引用された根拠はありません")}
          </p>
        )}
        {!!content.links?.length && (
          <ul className="memory-links">
            {content.links.map((link, index) => (
              <li key={index}>
                <span>{label(link.kind)}</span>
                <span>
                  {link.target} · r{link.revision}
                </span>
                <span>
                  {text("Weight", "重み")} {link.weight}
                </span>
              </li>
            ))}
          </ul>
        )}
        {content.occurred && (
          <p>
            {text("Occurred", "発生日時")}:{" "}
            <time dateTime={content.occurred.start}>
              {new Date(content.occurred.start).toLocaleString(locale)}
            </time>{" "}
            →{" "}
            <time dateTime={content.occurred.end}>
              {new Date(content.occurred.end).toLocaleString(locale)}
            </time>
          </p>
        )}
        {unit && (
          <p className="muted">
            {label("learned_at")}:{" "}
            <time dateTime={unit.learned_at}>
              {new Date(unit.learned_at).toLocaleString(locale)}
            </time>
            <br />
            ID: <span className="memory-id">{unit.id}</span>
          </p>
        )}
      </details>
    </div>
  );
}

/** Structured cards also cover paginated history, usage and engine-job records. */
export function MemoryRecord({
  value,
  units = [],
}: {
  value: unknown;
  units?: Unit[];
}) {
  const { locale, label, text } = useMemoryLabels();
  const labels = useRecordLabels();
  if (value === null || value === undefined)
    return <span className="muted">{text("Not recorded", "未記録")}</span>;
  if (typeof value === "boolean")
    return <span>{value ? text("Yes", "はい") : text("No", "いいえ")}</span>;
  if (typeof value === "number")
    return (
      <span className="memory-number">{value.toLocaleString(locale)}</span>
    );
  if (typeof value === "string") {
    const date = /^\d{4}-\d\d-\d\dT/.test(value) ? new Date(value) : null;
    return date && !Number.isNaN(date.getTime()) ? (
      <time dateTime={value}>{date.toLocaleString(locale)}</time>
    ) : (
      <span className="memory-value">
        {labels.get(value) ?? (names[value] ? label(value) : value)}
      </span>
    );
  }
  if (Array.isArray(value))
    return value.length ? (
      <ol className="memory-record-list">
        {value.map((item, index) => (
          <li key={index}>
            <MemoryRecord value={item} units={units} />
          </li>
        ))}
      </ol>
    ) : (
      <span className="muted">{text("None", "なし")}</span>
    );
  if (!record(value)) return null;
  if (typeof value.text === "string")
    return <MemoryContent content={value as Partial<Content>} units={units} />;
  if (
    typeof value.kind === "string" &&
    typeof value.id === "string" &&
    typeof value.revision === "number" &&
    ["unit", "run", "artifact", "message", "publication"].includes(value.kind)
  )
    return <EvidenceView value={value as Evidence} units={units} />;
  if (typeof value.id === "string" && typeof value.version === "string")
    return (
      <span>
        {labels.get(`${value.id}@${value.version}`) ??
          `${value.id} · ${value.version}`}
      </span>
    );
  return (
    <dl className="memory-fields">
      {Object.entries(value)
        .filter(([key]) => key !== "next")
        .map(([key, field]) => (
          <div key={key}>
            <dt>{label(key)}</dt>
            <dd>
              <MemoryRecord value={field} units={units} />
            </dd>
          </div>
        ))}
    </dl>
  );
}

export { unitAnchor };
