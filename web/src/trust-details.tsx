import { useState, type ReactNode } from "react";
import {
  ArrowRight,
  Award,
  Database,
  FileText,
  History,
  Info,
  LockKeyhole,
  Settings2,
  ShieldCheck,
} from "lucide-react";
import type { AuditPage, Inspection } from "./workbench";

type TrustTab =
  | "overview"
  | "policies"
  | "audit"
  | "certifications"
  | "incidents";
export function TrustEmpty({
  icon,
  title,
  children,
}: {
  icon: ReactNode;
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="trust-empty-state">
      <span className="trust-empty-icon">{icon}</span>
      <h3>{title}</h3>
      {children && <p>{children}</p>}
    </div>
  );
}

export function TrustCertifications({
  locale,
  navigate,
}: {
  locale: string;
  navigate: (tab: TrustTab) => void;
}) {
  const ja = locale === "ja-JP";
  return (
    <div className="trust-certifications">
      <section className="wb-card">
        <TrustEmpty
          icon={<Award size={56} />}
          title={
            ja
              ? "外部認証は未提供です"
              : "External certification is unavailable"
          }
        >
          {ja
            ? "外部の審査体制が整うまで、Trust評価・認証は行いません。"
            : "Trust assessments and certification await external review arrangements."}
        </TrustEmpty>
        <span className="trust-badge">{ja ? "未評価" : "Not assessed"}</span>
      </section>
      <div className="trust-evidence-links">
        {(
          [
            [
              "overview",
              <Settings2 size={26} />,
              ja ? "登録された構成" : "Registered configuration",
              ja
                ? "エージェントの構成、能力、動作設定を確認します。"
                : "Review the agent's registered configuration, capabilities and settings.",
            ],
            [
              "policies",
              <ShieldCheck size={26} />,
              ja ? "権限判定" : "Permission decisions",
              ja
                ? "指定した条件での許可・制限を確認します。"
                : "Inspect allowed and restricted components in a specific context.",
            ],
            [
              "audit",
              <FileText size={26} />,
              ja ? "監査記録" : "Audit evidence",
              ja
                ? "閲覧可能な操作履歴と記録された証拠を確認します。"
                : "Explore visible activity and recorded evidence.",
            ],
          ] as const
        ).map(([tab, icon, title, description]) => (
          <section className="wb-card" key={tab}>
            {icon}
            <h3>{title}</h3>
            <p>{description}</p>
            <button className="trust-link" onClick={() => navigate(tab)}>
              {ja
                ? "詳細を見る"
                : `Go to ${tab[0].toUpperCase()}${tab.slice(1)}`}
              <ArrowRight size={14} />
            </button>
          </section>
        ))}
      </div>
    </div>
  );
}

export function TrustSummary({
  inspection,
  version,
  locale,
  audit,
}: {
  inspection: Inspection | null;
  version: string;
  locale: string;
  audit: boolean;
}) {
  const ja = locale === "ja-JP";
  return (
    <aside className="trust-rail">
      <section className="wb-card">
        <h2>
          <ShieldCheck size={21} />
          {ja ? "Trustサマリー" : "Trust summary"}
        </h2>
        <small>{ja ? "総合評価" : "Overall assessment"}</small>
        <p className="trust-large">{ja ? "未評価" : "Not assessed"}</p>
        <p>
          {ja
            ? "外部の審査体制が整うまで、Trust評価・認証は行いません。登録・テスト・報告記録は外部評価とは区別されます。"
            : "Trust assessments and certification await external review arrangements. Registration, tests and incident reports are separate from external assessment."}
        </p>
      </section>
      <section className="wb-card">
        <h2>
          <Database size={21} />
          {ja ? "取得情報" : "Observation context"}
        </h2>
        <dl className="trust-facts">
          <div>
            <dt>{ja ? "取得元ノード" : "Source node"}</dt>
            <dd>{inspection?.source_node ?? "—"}</dd>
          </div>
          <div>
            <dt>{ja ? "バージョン" : "Version"}</dt>
            <dd>{version}</dd>
          </div>
          <div>
            <dt>{ja ? "取得日時" : "Observed at"}</dt>
            <dd>
              {inspection
                ? new Date(inspection.observed_at).toLocaleString(locale)
                : "—"}
            </dd>
          </div>
        </dl>
      </section>
      {audit && (
        <section className="wb-card">
          <h2>
            <Info size={21} />
            {ja ? "証拠の範囲" : "Evidence scope"}
          </h2>
          <h3>
            <Database size={17} />
            {ja ? "接続ノードのみ" : "Connected node only"}
          </h3>
          <p>
            {ja
              ? "現在接続しているノードの記録です。"
              : "Records are from the currently connected node."}
          </p>
          <h3>
            <LockKeyhole size={17} />
            {ja ? "閲覧権限に基づく記録" : "Access-limited records"}
          </h3>
          <p>
            {ja
              ? "すべての記録が閲覧できるとは限りません。"
              : "Only records visible with your access are included."}
          </p>
        </section>
      )}
    </aside>
  );
}

const eventKey = (item: AuditPage["items"][number]) =>
  JSON.stringify([item.source, item.kind, item.at, item.actor, item.details]);
export function TrustAudit({
  page,
  error,
  locale,
  offset,
  tenantControl,
  onOffset,
}: {
  page: AuditPage | null;
  error: string;
  locale: string;
  offset: number;
  tenantControl: ReactNode;
  onOffset: (offset: number) => void;
}) {
  const ja = locale === "ja-JP";
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const selected = page?.items.find((item) => eventKey(item) === selectedKey);
  return (
    <div className="trust-audit-stack">
      <section className="wb-card wb-audit">
        <header className="trust-panel-heading">
          <div>
            <h2>
              <History size={22} />
              {ja ? "監査履歴" : "Audit trail"}
            </h2>
            <p>
              {ja
                ? "このバージョンの閲覧可能な操作履歴を時系列で確認します。"
                : "Chronological record of visible activity for this agent version."}
            </p>
          </div>
          {tenantControl}
        </header>
        {page && (
          <p className="trust-caption">
            {page.source_boundary} ·{" "}
            {new Date(page.observed_at).toLocaleString(locale)}
          </p>
        )}
        {page?.items.length ? (
          <ol className="trust-timeline">
            {page.items.map((item, index) => (
              <li key={`${eventKey(item)}:${index}`}>
                <button
                  aria-pressed={selectedKey === eventKey(item)}
                  onClick={() => setSelectedKey(eventKey(item))}
                >
                  <span className="trust-event-title">
                    <FileText size={18} />
                    <strong>{item.kind}</strong>
                  </span>
                  <time dateTime={item.at}>
                    {new Date(item.at).toLocaleString(locale)}
                  </time>
                  <small>
                    {item.source} · {item.actor ?? "—"}
                  </small>
                </button>
              </li>
            ))}
          </ol>
        ) : (
          <TrustEmpty
            icon={<History size={36} />}
            title={
              error
                ? ja
                  ? "監査記録を取得できません"
                  : "Unable to load audit records"
                : page
                  ? ja
                    ? "閲覧可能な監査記録はありません"
                    : "No visible audit records"
                  : ja
                    ? "監査記録を取得中"
                    : "Loading audit records"
            }
          >
            {error ||
              (ja
                ? "このバージョンの閲覧可能な操作履歴がここに表示されます。"
                : "Visible activity for the selected agent version will appear here.")}
          </TrustEmpty>
        )}
        <div className="trust-audit-pagination">
          <button
            disabled={offset === 0}
            onClick={() => {
              setSelectedKey(null);
              onOffset(0);
            }}
          >
            {ja ? "最新に戻る" : "Back to latest"}
          </button>
          <span>
            {page
              ? page.items.length
                ? `${offset + 1}–${offset + page.items.length}`
                : "0"
              : "—"}
          </span>
          <button
            disabled={page?.next_offset == null}
            onClick={() => {
              if (page?.next_offset != null) {
                setSelectedKey(null);
                onOffset(page.next_offset);
              }
            }}
          >
            {ja ? "次の50件" : "Next 50"}
          </button>
        </div>
      </section>
      <section className="wb-card trust-event-details">
        <h2>
          <FileText size={20} />
          {ja ? "イベントの詳細" : "Event details"}
        </h2>
        {selected ? (
          <>
            <dl className="trust-facts">
              <div>
                <dt>{ja ? "取得元" : "Source"}</dt>
                <dd>{selected.source}</dd>
              </div>
              <div>
                <dt>{ja ? "操作主体" : "Actor"}</dt>
                <dd>{selected.actor ?? "—"}</dd>
              </div>
              <div>
                <dt>{ja ? "日時" : "Recorded at"}</dt>
                <dd>{new Date(selected.at).toLocaleString(locale)}</dd>
              </div>
            </dl>
            <pre>{JSON.stringify(selected.details, null, 2)}</pre>
          </>
        ) : (
          <TrustEmpty
            icon={<FileText size={30} />}
            title={ja ? "イベントが選択されていません" : "No event selected"}
          >
            {ja
              ? "監査履歴から記録を選択すると、詳細が表示されます。"
              : "Choose an audit record from the timeline to view its details here."}
          </TrustEmpty>
        )}
      </section>
    </div>
  );
}
