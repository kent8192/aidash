import { Button } from "./components/ui/button";
import type { ReactNode } from "react";
import {
  ArrowRight,
  Award,
  CircleAlert,
  Database,
  FileCheck2,
  FlaskConical,
  History,
  Layers,
  Link2,
  Settings2,
  ShieldCheck,
  Users,
  Wrench,
} from "lucide-react";
import type {
  AgentEntry,
  Inspection,
  Incident,
  PermissionContext,
} from "./workbench";

type Tab = "policies" | "audit" | "certifications" | "incidents";
export function TrustOverview({
  agent,
  inspection,
  incidents,
  incidentsLoaded,
  permissions,
  locale,
  navigate,
}: {
  agent: AgentEntry;
  inspection: Inspection | null;
  incidents: Incident[];
  incidentsLoaded: boolean;
  permissions: PermissionContext | null;
  locale: string;
  navigate: (tab: Tab) => void;
}) {
  const ja = locale === "ja-JP";
  const text = (en: string, jp: string) => (ja ? jp : en);
  const unknown = text("Not assessed", "未評価");
  const empty = text(
    "No records visible with your access.",
    "この権限で閲覧できる記録はありません。",
  );
  const pending = text("Not available", "未取得");
  const date = (value: string) => new Date(value).toLocaleDateString(locale);
  const open = incidents.filter(
    (item) => !item.archived && item.status === "open",
  );
  const rows = permissions?.rows ?? [];
  const allowed = rows.filter((row) => row.effective_for_component).length;
  const tests = inspection?.test_evidence ?? [];
  const title = (ref: { id: string; version: string }) =>
    `${ref.id} @ ${ref.version}`;
  const dependencies = [
    ...agent.config.bindings.map((binding) => ({
      kind: binding.kind,
      reference: binding.target,
    })),
  ];
  const badge = (value: ReactNode, tone = "neutral") => (
    <span className={`trust-badge ${tone}`}>{value}</span>
  );
  const link = (tab: Tab, caption = text("View details", "詳細を見る")) => (
    <Button
      variant="outline"
      className="trust-link"
      onClick={() => navigate(tab)}
    >
      {caption}
      <ArrowRight size={13} />
    </Button>
  );
  const card = (
    name: string,
    icon: ReactNode,
    body: ReactNode,
    footer?: ReactNode,
    className = "",
  ) => (
    <section className={`wb-card trust-card ${className}`}>
      <h2>
        {icon}
        {name}
      </h2>
      <div className="trust-card-body">{body}</div>
      {footer}
    </section>
  );
  const permissionBody = permissions ? (
    <>
      <p className="trust-large">
        {allowed}
        <span> / {rows.length}</span>
      </p>
      <p>
        {text(
          "Components allowed in checked context",
          "確認条件で許可されたコンポーネント",
        )}
      </p>
      <meter
        min={0}
        max={Math.max(rows.length, 1)}
        value={allowed}
        aria-label={text("Allowed components", "許可コンポーネント")}
      />
      <dl className="trust-facts">
        <div>
          <dt>{text("Allowed", "許可")}</dt>
          <dd>{allowed}</dd>
        </div>
        <div>
          <dt>{text("Restricted", "制限")}</dt>
          <dd>{rows.length - allowed}</dd>
        </div>
      </dl>
      <p className="trust-caption">
        {permissions.tenant} / {permissions.subject}
        <br />
        {permissions.workspace_id ||
          text("No workspace selected", "ワークスペース未選択")}{" "}
        · {date(permissions.observed_at)}
      </p>
    </>
  ) : (
    <>
      <p className="trust-large">{text("Not checked", "未確認")}</p>
      <p>
        {text(
          "Select a tenant, subject and optional workspace in Policies to inspect effective access.",
          "ポリシーでテナント・主体・必要なワークスペースを指定すると、実効権限を確認できます。",
        )}
      </p>
    </>
  );
  const incidentList = (
    <>
      {!incidentsLoaded ? (
        <p>{pending}</p>
      ) : open.length ? (
        <ul className="trust-list">
          {open.slice(0, 3).map((item) => (
            <li key={item.id}>
              <div className="trust-row">
                {badge(
                  item.severity,
                  item.severity === "high" || item.severity === "critical"
                    ? "danger"
                    : "warning",
                )}
                <time>{date(item.created_at)}</time>
              </div>
              <p className="trust-clamp">{item.notes}</p>
              <small>{item.id}</small>
            </li>
          ))}
        </ul>
      ) : (
        <p>
          {text(
            "No open incidents visible with your access.",
            "この権限で閲覧できる未解決インシデントはありません。",
          )}
        </p>
      )}
    </>
  );
  return (
    <div className="trust-overview">
      <div className="trust-main">
        <div className="trust-metrics">
          {[
            [
              <ShieldCheck size={19} />,
              text("Trust score", "Trustスコア"),
              unknown,
              text("External assessment", "外部審査"),
            ],
            [
              <ShieldCheck size={19} />,
              text("Effective access", "実効権限"),
              permissions
                ? `${allowed} / ${rows.length}`
                : text("Not checked", "未確認"),
              text("Context-specific", "指定条件での判定"),
            ],
            [
              <Award size={19} />,
              text("Certifications", "認証"),
              text("Unavailable", "未提供"),
              text("External review pending", "外部審査体制待ち"),
            ],
            [
              <CircleAlert size={19} />,
              text("Open issues", "未解決の問題"),
              incidentsLoaded ? open.length : "—",
              text("Visible incidents", "閲覧可能なインシデント"),
            ],
            [
              <History size={19} />,
              text("Last observed", "最終取得"),
              inspection ? date(inspection.observed_at) : "—",
              text("Connected-node snapshot", "接続ノードの記録"),
            ],
          ].map(([icon, name, value, caption], index) => (
            <section className="wb-card trust-metric" key={index}>
              <h2>
                {icon}
                {name}
              </h2>
              <strong>{value}</strong>
              <small>{caption}</small>
            </section>
          ))}
        </div>
        <div className="trust-card-grid">
          {card(
            text("Policy coverage", "ポリシー適用状況"),
            <ShieldCheck size={21} />,
            permissionBody,
            link("policies", text("View policies", "ポリシーを見る")),
          )}
          {card(
            text("Certifications", "認証"),
            <Award size={21} />,
            <>
              <div className="trust-unassessed">
                <Award size={32} />
                {badge(unknown)}
              </div>
              <p>
                {text(
                  "Certification records are not available until external review arrangements exist.",
                  "外部の審査体制が整うまで、認証の評価・記録は提供されません。",
                )}
              </p>
            </>,
            link("certifications", text("View certifications", "認証を見る")),
          )}
          {card(
            text("Lineage / Provenance", "来歴・出自"),
            <Layers size={21} />,
            <dl className="trust-facts">
              <div>
                <dt>{text("Model", "モデル")}</dt>
                <dd>{title(agent.config.model)}</dd>
              </div>
              <div>
                <dt>{text("Skills", "スキル")}</dt>
                <dd>
                  {
                    agent.config.bindings.filter((b) => b.kind === "skill")
                      .length
                  }
                </dd>
              </div>
              <div>
                <dt>{text("Source node", "取得元ノード")}</dt>
                <dd>{inspection?.source_node ?? pending}</dd>
              </div>
              <div>
                <dt>{text("Version", "バージョン")}</dt>
                <dd>{agent.version}</dd>
              </div>
              <div>
                <dt>{text("Observed", "取得日時")}</dt>
                <dd>{inspection ? date(inspection.observed_at) : pending}</dd>
              </div>
            </dl>,
            link("audit", text("View audit trail", "監査記録を見る")),
          )}
          {card(
            text("Data access scope", "データアクセス範囲"),
            <Database size={21} />,
            <>
              <ul className="trust-list">
                {(
                  [
                    [
                      text("Source bindings", "SourceのBinding"),
                      agent.config.bindings.some((b) => b.kind === "source"),
                    ],
                    [
                      text("Memory write", "メモリ書き込み"),
                      !agent.config.remove_default.includes("memory_write"),
                    ],
                    [
                      text("Memory bindings", "MemoryのBinding"),
                      agent.config.bindings.some((b) => b.kind === "memory"),
                    ],
                  ] as const
                ).map(([name, enabled]) => (
                  <li className="trust-row" key={name}>
                    <span>{name}</span>
                    {badge(
                      enabled === undefined
                        ? text("Default", "既定")
                        : enabled
                          ? text("Requested", "要求あり")
                          : text("Disabled", "無効"),
                    )}
                  </li>
                ))}
              </ul>
              <h3>{text("Declared capabilities", "宣言済みcapability")}</h3>
              {agent.capabilities.length ? (
                <ul className="trust-list">
                  {agent.capabilities.map((capability) => (
                    <li key={capability}>{capability}</li>
                  ))}
                </ul>
              ) : (
                <p>
                  {text(
                    "No capabilities declared.",
                    "宣言済みcapabilityはありません。",
                  )}
                </p>
              )}
              <p className="trust-caption">
                {text(
                  "Agent configuration; effective access depends on policy.",
                  "エージェントの設定です。実効権限はポリシーに依存します。",
                )}
              </p>
            </>,
            link("policies"),
          )}
          {card(
            text("Autonomy settings", "自律動作の設定"),
            <Settings2 size={21} />,
            <>
              <dl className="trust-facts">
                {(
                  [
                    [
                      text("Automatic task creation", "タスクの自動作成"),
                      !agent.config.remove_default.includes("task_create"),
                    ],
                    [
                      text("Automatic delegation", "自動委任"),
                      !agent.config.remove_default.includes("task_delegate"),
                    ],
                  ] as const
                ).map(([name, enabled]) => (
                  <div key={name}>
                    <dt>{name}</dt>
                    <dd>
                      {badge(
                        enabled === undefined
                          ? text("Default", "既定")
                          : enabled
                            ? text("Enabled", "有効")
                            : text("Disabled", "無効"),
                      )}
                    </dd>
                  </div>
                ))}
              </dl>
              <p className="trust-caption">
                {text(
                  "Registered configuration; execution remains subject to policy.",
                  "登録済みの設定です。実行にはポリシーによる許可が必要です。",
                )}
              </p>
            </>,
          )}
          {card(
            text("Configured tools and skills", "設定済みツール・スキル"),
            <Wrench size={21} />,
            <>
              {dependencies.length ? (
                <ul className="trust-list">
                  {dependencies.map(({ kind, reference }) => {
                    const decision = rows.find(
                      (row) =>
                        row.kind === kind &&
                        row.reference.id === reference.id &&
                        row.reference.version === reference.version,
                    );
                    return (
                      <li
                        className="trust-row"
                        key={`${kind}:${title(reference)}`}
                      >
                        <span>
                          <small>
                            {
                              (
                                {
                                  tool: text("Tool", "ツール"),
                                  skill: text("Skill", "スキル"),
                                  bundle: text("Bundle", "バンドル"),
                                  source: "Source",
                                  memory: "Memory",
                                } as Record<string, string>
                              )[kind]
                            }
                          </small>
                          <br />
                          {title(reference)}
                        </span>
                        {badge(
                          decision
                            ? decision.effective_for_component
                              ? text("Allowed", "許可")
                              : text("Restricted", "制限")
                            : text("Not checked", "未確認"),
                          decision
                            ? decision.effective_for_component
                              ? "success"
                              : "danger"
                            : "neutral",
                        )}
                      </li>
                    );
                  })}
                </ul>
              ) : (
                <p>
                  {text(
                    "No tools or skills configured for this version.",
                    "このバージョンにツール・スキルは設定されていません。",
                  )}
                </p>
              )}
            </>,
            link("policies", text("Inspect permissions", "権限を確認")),
          )}
          {card(
            text("Human approvals", "人による承認"),
            <Users size={21} />,
            <>
              <p className="trust-large">{text("Not available", "未提供")}</p>
              <p>
                {text(
                  "No approval workflow evidence is included in this agent inspection. Registration does not imply approval.",
                  "このエージェントの確認情報には承認ワークフローの記録が含まれていません。登録は承認を意味しません。",
                )}
              </p>
            </>,
          )}
          {card(
            text("Permission matrix", "権限マトリクス"),
            <FileCheck2 size={21} />,
            permissions ? (
              <div className="trust-table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>
                        {text("Component / action", "コンポーネント・操作")}
                      </th>
                      <th>{text("Decision", "判定")}</th>
                      <th>{text("Checked", "確認日")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((row) => (
                      <tr key={`${title(row.reference)}:${row.action}`}>
                        <td>
                          {title(row.reference)}
                          <small>{row.action}</small>
                        </td>
                        <td>
                          {badge(
                            row.effective_for_component
                              ? text("Allowed", "許可")
                              : text("Restricted", "制限"),
                            row.effective_for_component ? "success" : "danger",
                          )}
                        </td>
                        <td>{date(permissions.observed_at)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : (
              <p>
                {text(
                  "Check a policy context to see catalog, policy and Registry access decisions for this version.",
                  "ポリシーで条件を指定すると、このバージョンのカタログ・ポリシー・Registry参照に基づく判定を表示します。",
                )}
              </p>
            ),
            link("policies", text("View full matrix", "マトリクスを見る")),
            "trust-matrix",
          )}
          {card(
            text("Risk signals", "リスク情報"),
            <CircleAlert size={21} />,
            <>
              {incidentList}
              <p className="trust-caption">
                {text(
                  "Reported incidents only. No automated risk score is available.",
                  "報告されたインシデントのみ表示します。自動リスク評価は未提供です。",
                )}
              </p>
            </>,
            link("incidents"),
          )}
        </div>
      </div>
      <aside className="trust-rail">
        {card(
          text("Trust summary", "Trustサマリー"),
          <ShieldCheck size={21} />,
          <>
            <small>{text("Overall assessment", "総合評価")}</small>
            <p className="trust-large">{unknown}</p>
            <p>
              {text(
                "This overview shows registered facts and visible records. Trust scoring and certification await external review arrangements.",
                "登録情報と閲覧可能な記録を表示しています。Trustスコアや認証は外部審査体制の整備待ちです。",
              )}
            </p>
          </>,
        )}
        {card(
          text("Open issues", "未解決の問題"),
          <CircleAlert size={21} />,
          incidentList,
          link("incidents", text("View all issues", "すべての問題を見る")),
        )}
        {card(
          text("Latest test evidence", "最新のテスト記録"),
          <FlaskConical size={21} />,
          <>
            {!inspection ? (
              <p>{pending}</p>
            ) : tests.length ? (
              <ul className="trust-list">
                {tests.slice(0, 3).map((test) => (
                  <li key={test.session_id}>
                    <div className="trust-row">
                      <strong>{test.mode}</strong>
                      {badge(test.status)}
                    </div>
                    <p>
                      r{test.draft_revision}
                      {test.profile_id ? ` · ${test.profile_id}` : ""}
                    </p>
                    <time>{date(test.created_at)}</time>
                    {test.expired_at && (
                      <p>{text("Payload expired", "本文は期限切れ")}</p>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <p>{empty}</p>
            )}
            <p className="trust-caption">
              {text(
                "Test records are not external attestations.",
                "テスト記録は外部機関による証明ではありません。",
              )}
              {inspection?.test_evidence_truncated &&
                text(" Latest 100 records only.", " 最新100件が対象です。")}
            </p>
          </>,
        )}
        {card(
          text("Impacted workspaces", "利用ワークスペース"),
          <Users size={21} />,
          <>
            {!inspection ? (
              <p>{pending}</p>
            ) : inspection.workspaces.length ? (
              <ul className="trust-list">
                {inspection.workspaces.map((workspace) => (
                  <li key={workspace.workspace_id}>
                    <div className="trust-row">
                      <Link2 size={16} />
                      <strong>{workspace.title}</strong>
                    </div>
                    <p>
                      {workspace.current
                        ? text("Current", "現在利用中")
                        : text("Past", "過去の利用")}{" "}
                      · {date(workspace.latest_run_at)}
                    </p>
                  </li>
                ))}
              </ul>
            ) : (
              <p>{empty}</p>
            )}
            {inspection?.usage_truncated && (
              <p className="trust-caption">
                {text(
                  "Based on the latest 500 visible runs.",
                  "閲覧可能な最新500実行に基づきます。",
                )}
              </p>
            )}
          </>,
        )}
      </aside>
    </div>
  );
}
