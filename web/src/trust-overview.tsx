import { Button } from "./components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { useId, type ReactNode } from "react";
import { ArrowRight } from "lucide-react";
import type {
  AgentEntry,
  Inspection,
  Incident,
  PermissionContext,
} from "./workbench";
import {
  Facts,
  Hint,
  InspectorSection,
  Metric,
  MetricRow,
} from "./components/patterns";
import { FactBadge } from "./trust-details";

type Tab = "policies" | "audit" | "certifications" | "incidents";

/** Hairline block of the overview list grid. */
function Block({
  title,
  link,
  className = "",
  children,
}: {
  title: string;
  link?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className={`grid min-w-0 grid-cols-[minmax(0,1fr)] content-start gap-3 border-t border-border py-5 ${className}`}
    >
      <div className="flex min-h-7 items-center justify-between gap-3">
        <h2 id={id} className="text-[13px] font-semibold text-foreground">
          {title}
        </h2>
        {link}
      </div>
      {children}
    </section>
  );
}

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
  const link = (tab: Tab, caption = text("View details", "詳細を見る")) => (
    <Button
      variant="ghost"
      size="sm"
      type="button"
      className="text-brand hover:text-brand"
      onClick={() => navigate(tab)}
    >
      {caption}
      <ArrowRight />
    </Button>
  );
  const incidentList = !incidentsLoaded ? (
    <Hint>{pending}</Hint>
  ) : open.length ? (
    <ul className="divide-y divide-border">
      {open.slice(0, 3).map((item) => (
        <li key={item.id} className="grid gap-1 py-2 first:pt-0">
          <div className="flex items-center gap-2">
            <FactBadge
              tone={
                item.severity === "high" || item.severity === "critical"
                  ? "danger"
                  : "warning"
              }
            >
              {item.severity}
            </FactBadge>
            <time className="font-mono text-[11px] text-faint tabular">
              {date(item.created_at)}
            </time>
          </div>
          <p className="line-clamp-2 text-[13px] text-foreground">
            {item.notes}
          </p>
          <span className="font-mono text-[11px] text-faint">{item.id}</span>
        </li>
      ))}
    </ul>
  ) : (
    <Hint>
      {text(
        "No open incidents visible with your access.",
        "この権限で閲覧できる未解決インシデントはありません。",
      )}
    </Hint>
  );
  const kindLabel: Record<string, string> = {
    tool: text("Tool", "ツール"),
    skill: text("Skill", "スキル"),
    bundle: text("Bundle", "バンドル"),
    source: "Source",
    memory: "Memory",
  };
  return (
    <div className="trust-overview grid min-w-0 grid-cols-[minmax(0,1fr)]">
      <MetricRow label={text("Trust facts", "Trustの事実")}>
        <Metric
          label={text("Trust score", "Trustスコア")}
          value={unknown}
          caption={text("External assessment", "外部審査")}
        />
        <Metric
          label={text("Effective access", "実効権限")}
          value={
            permissions
              ? `${allowed} / ${rows.length}`
              : text("Not checked", "未確認")
          }
          caption={text("Context-specific", "指定条件での判定")}
        />
        <Metric
          label={text("Certifications", "認証")}
          value={text("Unavailable", "未提供")}
          caption={text("External review pending", "外部審査体制待ち")}
        />
        <Metric
          label={text("Open issues", "未解決の問題")}
          tone={incidentsLoaded && open.length ? "danger" : undefined}
          value={incidentsLoaded ? open.length : "—"}
          caption={text("Visible incidents", "閲覧可能なインシデント")}
        />
        <Metric
          className="col-span-2 xl:col-span-1"
          label={text("Last observed", "最終取得")}
          value={inspection ? date(inspection.observed_at) : "—"}
          caption={text("Connected-node snapshot", "接続ノードの記録")}
        />
      </MetricRow>
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] lg:grid-cols-[minmax(0,1fr)_360px]">
        <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] content-start px-4 pb-6 md:px-6 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] xl:gap-x-8">
          <Block
            title={text("Policy coverage", "ポリシー適用状況")}
            link={link("policies", text("View policies", "ポリシーを見る"))}
          >
            {permissions ? (
              <>
                <p className="font-mono text-[22px] leading-7 font-medium tabular text-foreground">
                  {allowed}
                  <span className="text-faint"> / {rows.length}</span>
                </p>
                <Hint>
                  {text(
                    "Components allowed in checked context",
                    "確認条件で許可されたコンポーネント",
                  )}
                </Hint>
                <meter
                  className="h-1.5 w-full"
                  min={0}
                  max={Math.max(rows.length, 1)}
                  value={allowed}
                  aria-label={text("Allowed components", "許可コンポーネント")}
                />
                <Facts
                  items={[
                    [
                      text("Allowed", "許可"),
                      <span className="font-mono tabular">{allowed}</span>,
                    ],
                    [
                      text("Restricted", "制限"),
                      <span className="font-mono tabular">
                        {rows.length - allowed}
                      </span>,
                    ],
                  ]}
                />
                <p className="font-mono text-[11px] text-faint">
                  {permissions.tenant} / {permissions.subject} ·{" "}
                  {permissions.workspace_id ||
                    text("No workspace selected", "ワークスペース未選択")}{" "}
                  · {date(permissions.observed_at)}
                </p>
              </>
            ) : (
              <>
                <p className="text-[15px] font-semibold text-foreground">
                  {text("Not checked", "未確認")}
                </p>
                <Hint>
                  {text(
                    "Select a tenant, subject and optional workspace in Policies to inspect effective access.",
                    "ポリシーでテナント・主体・必要なワークスペースを指定すると、実効権限を確認できます。",
                  )}
                </Hint>
              </>
            )}
          </Block>
          <Block
            title={text("Certifications", "認証")}
            link={link(
              "certifications",
              text("View certifications", "認証を見る"),
            )}
          >
            <div>
              <FactBadge>{unknown}</FactBadge>
            </div>
            <Hint>
              {text(
                "Certification records are not available until external review arrangements exist.",
                "外部の審査体制が整うまで、認証の評価・記録は提供されません。",
              )}
            </Hint>
          </Block>
          <Block
            title={text("Lineage / Provenance", "来歴・出自")}
            link={link("audit", text("View audit trail", "監査記録を見る"))}
          >
            <Facts
              items={[
                [
                  text("Model", "モデル"),
                  <span className="font-mono text-xs">
                    {title(agent.config.model)}
                  </span>,
                ],
                [
                  text("Skills", "スキル"),
                  <span className="font-mono tabular">
                    {
                      agent.config.bindings.filter((b) => b.kind === "skill")
                        .length
                    }
                  </span>,
                ],
                [
                  text("Source node", "取得元ノード"),
                  <span className="font-mono text-xs">
                    {inspection?.source_node ?? pending}
                  </span>,
                ],
                [
                  text("Version", "バージョン"),
                  <span className="font-mono text-xs">{agent.version}</span>,
                ],
                [
                  text("Observed", "取得日時"),
                  <span className="font-mono text-xs tabular">
                    {inspection ? date(inspection.observed_at) : pending}
                  </span>,
                ],
              ]}
            />
          </Block>
          <Block
            title={text("Data access scope", "データアクセス範囲")}
            link={link("policies")}
          >
            <ul className="divide-y divide-border">
              {(
                [
                  [
                    text("Source bindings", "SourceのBinding"),
                    agent.config.bindings.some((b) => b.kind === "source"),
                  ],
                  [
                    text("Memory write", "メモリ書き込み"),
                    !agent.config.remove_default.includes("memory_mutate"),
                  ],
                  [
                    text("Memory bindings", "MemoryのBinding"),
                    agent.config.bindings.some((b) => b.kind === "memory"),
                  ],
                ] as const
              ).map(([name, enabled]) => (
                <li
                  className="trust-row flex items-center justify-between gap-3 py-1.5 text-[13px]"
                  key={name}
                >
                  <span>{name}</span>
                  <FactBadge>
                    {enabled
                      ? text("Requested", "要求あり")
                      : text("Disabled", "無効")}
                  </FactBadge>
                </li>
              ))}
            </ul>
            <h3 className="text-xs font-semibold text-foreground">
              {text("Declared capabilities", "宣言済みcapability")}
            </h3>
            {agent.capabilities.length ? (
              <ul className="flex flex-wrap gap-1.5">
                {agent.capabilities.map((capability) => (
                  <li
                    key={capability}
                    className="rounded-sm bg-raised px-1.5 py-0.5 font-mono text-xs text-foreground"
                  >
                    {capability}
                  </li>
                ))}
              </ul>
            ) : (
              <Hint>
                {text(
                  "No capabilities declared.",
                  "宣言済みcapabilityはありません。",
                )}
              </Hint>
            )}
            <Hint>
              {text(
                "Agent configuration; effective access depends on policy.",
                "エージェントの設定です。実効権限はポリシーに依存します。",
              )}
            </Hint>
          </Block>
          <Block title={text("Autonomy settings", "自律動作の設定")}>
            <Facts
              items={(
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
              ).map(([name, enabled]) => [
                name,
                <FactBadge tone={enabled ? "brand" : "neutral"}>
                  {enabled ? text("Enabled", "有効") : text("Disabled", "無効")}
                </FactBadge>,
              ])}
            />
            <Hint>
              {text(
                "Registered configuration; execution remains subject to policy.",
                "登録済みの設定です。実行にはポリシーによる許可が必要です。",
              )}
            </Hint>
          </Block>
          <Block
            title={text("Configured tools and skills", "設定済みツール・スキル")}
            link={link("policies", text("Inspect permissions", "権限を確認"))}
          >
            {agent.config.bindings.length ? (
              <ul className="divide-y divide-border">
                {agent.config.bindings.map(({ kind, target: reference }) => {
                  const decision = rows.find(
                    (row) =>
                      row.kind === kind &&
                      row.reference.id === reference.id &&
                      row.reference.version === reference.version,
                  );
                  return (
                    <li
                      className="trust-row flex items-center justify-between gap-3 py-1.5"
                      key={`${kind}:${title(reference)}`}
                    >
                      <span className="grid min-w-0">
                        <small className="text-[11px] text-faint">
                          {kindLabel[kind]}
                        </small>
                        <span className="truncate font-mono text-xs text-foreground">
                          {title(reference)}
                        </span>
                      </span>
                      <FactBadge
                        tone={
                          decision
                            ? decision.effective_for_component
                              ? "success"
                              : "danger"
                            : "neutral"
                        }
                      >
                        {decision
                          ? decision.effective_for_component
                            ? text("Allowed", "許可")
                            : text("Restricted", "制限")
                          : text("Not checked", "未確認")}
                      </FactBadge>
                    </li>
                  );
                })}
              </ul>
            ) : (
              <Hint>
                {text(
                  "No tools or skills configured for this version.",
                  "このバージョンにツール・スキルは設定されていません。",
                )}
              </Hint>
            )}
          </Block>
          <Block title={text("Human approvals", "人による承認")}>
            <p className="text-[15px] font-semibold text-foreground">
              {text("Not available", "未提供")}
            </p>
            <Hint>
              {text(
                "No approval workflow evidence is included in this agent inspection. Registration does not imply approval.",
                "このエージェントの確認情報には承認ワークフローの記録が含まれていません。登録は承認を意味しません。",
              )}
            </Hint>
          </Block>
          <Block
            title={text("Risk signals", "リスク情報")}
            link={link("incidents")}
          >
            {incidentList}
            <Hint>
              {text(
                "Reported incidents only. No automated risk score is available.",
                "報告されたインシデントのみ表示します。自動リスク評価は未提供です。",
              )}
            </Hint>
          </Block>
          <Block
            title={text("Permission matrix", "権限マトリクス")}
            link={link("policies", text("View full matrix", "マトリクスを見る"))}
            className="trust-matrix xl:col-span-2"
          >
            {permissions ? (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>
                      {text("Component / action", "コンポーネント・操作")}
                    </TableHead>
                    <TableHead>{text("Decision", "判定")}</TableHead>
                    <TableHead>{text("Checked", "確認日")}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => (
                    <TableRow key={`${title(row.reference)}:${row.action}`}>
                      <TableCell>
                        <span className="block font-mono text-xs">
                          {title(row.reference)}
                        </span>
                        <span className="block font-mono text-[11px] text-faint">
                          {row.action}
                        </span>
                      </TableCell>
                      <TableCell>
                        <FactBadge
                          tone={
                            row.effective_for_component ? "success" : "danger"
                          }
                        >
                          {row.effective_for_component
                            ? text("Allowed", "許可")
                            : text("Restricted", "制限")}
                        </FactBadge>
                      </TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        {date(permissions.observed_at)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            ) : (
              <Hint>
                {text(
                  "Check a policy context to see catalog, policy and Registry access decisions for this version.",
                  "ポリシーで条件を指定すると、このバージョンのカタログ・ポリシー・Registry参照に基づく判定を表示します。",
                )}
              </Hint>
            )}
          </Block>
        </div>
        <aside
          aria-label={text("Trust summary", "Trustサマリー")}
          className="min-w-0 border-t border-border bg-surface lg:border-t-0 lg:border-l"
        >
          <InspectorSection title={text("Trust summary", "Trustサマリー")}>
            <Facts
              items={[
                [
                  text("Overall assessment", "総合評価"),
                  <FactBadge>{unknown}</FactBadge>,
                ],
              ]}
            />
            <Hint>
              {text(
                "This overview shows registered facts and visible records. Trust scoring and certification await external review arrangements.",
                "登録情報と閲覧可能な記録を表示しています。Trustスコアや認証は外部審査体制の整備待ちです。",
              )}
            </Hint>
          </InspectorSection>
          <InspectorSection
            title={text("Open issues", "未解決の問題")}
            action={link(
              "incidents",
              text("View all issues", "すべての問題を見る"),
            )}
          >
            {incidentList}
          </InspectorSection>
          <InspectorSection
            title={text("Latest test evidence", "最新のテスト記録")}
          >
            {!inspection ? (
              <Hint>{pending}</Hint>
            ) : tests.length ? (
              <ul className="divide-y divide-border">
                {tests.slice(0, 3).map((test) => (
                  <li key={test.session_id} className="grid gap-1 py-2 first:pt-0">
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[13px] font-medium">{test.mode}</span>
                      <FactBadge>{test.status}</FactBadge>
                    </div>
                    <p className="font-mono text-[11px] text-faint">
                      r{test.draft_revision}
                      {test.profile_id ? ` · ${test.profile_id}` : ""} ·{" "}
                      {date(test.created_at)}
                    </p>
                    {test.expired_at && (
                      <Hint>{text("Payload expired", "本文は期限切れ")}</Hint>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <Hint>{empty}</Hint>
            )}
            <p className="text-[11px] text-faint">
              {text(
                "Test records are not external attestations.",
                "テスト記録は外部機関による証明ではありません。",
              )}
              {inspection?.test_evidence_truncated &&
                text(" Latest 100 records only.", " 最新100件が対象です。")}
            </p>
          </InspectorSection>
          <InspectorSection title={text("Impacted workspaces", "利用ワークスペース")}>
            {!inspection ? (
              <Hint>{pending}</Hint>
            ) : inspection.workspaces.length ? (
              <ul className="divide-y divide-border">
                {inspection.workspaces.map((workspace) => (
                  <li
                    key={workspace.workspace_id}
                    className="flex items-center justify-between gap-3 py-1.5"
                  >
                    <span className="truncate text-[13px] font-medium">
                      {workspace.title}
                    </span>
                    <span className="shrink-0 text-[11px] text-faint">
                      {workspace.current
                        ? text("Current", "現在利用中")
                        : text("Past", "過去の利用")}{" "}
                      ·{" "}
                      <span className="font-mono tabular">
                        {date(workspace.latest_run_at)}
                      </span>
                    </span>
                  </li>
                ))}
              </ul>
            ) : (
              <Hint>{empty}</Hint>
            )}
            {inspection?.usage_truncated && (
              <p className="text-[11px] text-faint">
                {text(
                  "Based on the latest 500 visible runs.",
                  "閲覧可能な最新500実行に基づきます。",
                )}
              </p>
            )}
          </InspectorSection>
        </aside>
      </div>
    </div>
  );
}
