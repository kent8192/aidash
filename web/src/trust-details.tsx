import { Badge as ToneBadge } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { useState, type ReactNode } from "react";
import {
  ArrowRight,
  Award,
  Database,
  FileText,
  History,
  LockKeyhole,
  Settings2,
  ShieldCheck,
} from "lucide-react";
import { Field, JsonView, type StatusTone } from "./ui";
import type { AuditPage, Inspection } from "./workbench";
import {
  EmptyState,
  Facts,
  Hint,
  Inspector,
  InspectorSection,
  MainColumn,
  Section,
} from "./components/patterns";

/** Labelled fact chip used by Trust (`trust-badge` is a test hook). */
export function FactBadge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: StatusTone;
}) {
  return (
    <ToneBadge tone={tone} className="trust-badge">
      {children}
    </ToneBadge>
  );
}

type TrustTab =
  | "overview"
  | "policies"
  | "audit"
  | "certifications"
  | "incidents";

export function TrustCertifications({
  locale,
  navigate,
}: {
  locale: string;
  navigate: (tab: TrustTab) => void;
}) {
  const ja = locale === "ja-JP";
  return (
    <>
      <Section title={ja ? "認証" : "Certifications"}>
        <EmptyState
          icon={<Award />}
          title={
            ja
              ? "外部認証は未提供です"
              : "External certification is unavailable"
          }
          action={<FactBadge>{ja ? "未評価" : "Not assessed"}</FactBadge>}
        >
          {ja
            ? "外部の審査体制が整うまで、Trust評価・認証は行いません。"
            : "Trust assessments and certification await external review arrangements."}
        </EmptyState>
      </Section>
      <Section
        title={ja ? "確認できる事実の記録" : "Factual records"}
        description={
          ja
            ? "認証の代わりに、登録内容・権限判定・監査記録を確認できます。"
            : "Instead of a certification, review the registered configuration, permission decisions and audit records."
        }
      >
        <ul className="divide-y divide-border border-y border-border">
          {(
            [
              [
                "overview",
                <Settings2 />,
                ja ? "登録された構成" : "Registered configuration",
                ja
                  ? "エージェントの構成、能力、動作設定を確認します。"
                  : "Review the agent's registered configuration, capabilities and settings.",
              ],
              [
                "policies",
                <ShieldCheck />,
                ja ? "権限判定" : "Permission decisions",
                ja
                  ? "指定した条件での許可・制限を確認します。"
                  : "Inspect allowed and restricted components in a specific context.",
              ],
              [
                "audit",
                <FileText />,
                ja ? "監査記録" : "Audit evidence",
                ja
                  ? "閲覧可能な操作履歴と記録された証拠を確認します。"
                  : "Explore visible activity and recorded evidence.",
              ],
            ] as const
          ).map(([tab, icon, title, description]) => (
            <li
              key={tab}
              className="flex flex-wrap items-center gap-x-3 gap-y-2 py-3"
            >
              <span aria-hidden className="text-faint [&_svg]:size-4">
                {icon}
              </span>
              <div className="min-w-0 flex-1 basis-56">
                <h3 className="text-[13px] font-medium text-foreground">
                  {title}
                </h3>
                <p className="text-xs text-muted-foreground">{description}</p>
              </div>
              <Button
                variant="outline"
                size="sm"
                type="button"
                onClick={() => navigate(tab)}
              >
                {ja
                  ? "詳細を見る"
                  : `Go to ${tab[0].toUpperCase()}${tab.slice(1)}`}
                <ArrowRight />
              </Button>
            </li>
          ))}
        </ul>
      </Section>
    </>
  );
}

/** Inspector sections shared by the Trust detail tabs. */
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
    <>
      <InspectorSection title={ja ? "Trustサマリー" : "Trust summary"}>
        <Facts
          items={[
            [
              ja ? "総合評価" : "Overall assessment",
              <FactBadge>{ja ? "未評価" : "Not assessed"}</FactBadge>,
            ],
          ]}
        />
        <Hint>
          {ja
            ? "外部の審査体制が整うまで、Trust評価・認証は行いません。登録・テスト・報告記録は外部評価とは区別されます。"
            : "Trust assessments and certification await external review arrangements. Registration, tests and incident reports are separate from external assessment."}
        </Hint>
      </InspectorSection>
      <InspectorSection title={ja ? "取得情報" : "Observation context"}>
        <Facts
          items={[
            [
              ja ? "取得元ノード" : "Source node",
              <span className="font-mono text-xs">
                {inspection?.source_node ?? "—"}
              </span>,
            ],
            [
              ja ? "バージョン" : "Version",
              <span className="font-mono text-xs">{version}</span>,
            ],
            [
              ja ? "取得日時" : "Observed at",
              <span className="font-mono text-xs tabular">
                {inspection
                  ? new Date(inspection.observed_at).toLocaleString(locale)
                  : "—"}
              </span>,
            ],
          ]}
        />
      </InspectorSection>
      {audit && (
        <InspectorSection title={ja ? "証拠の範囲" : "Evidence scope"}>
          <ul className="grid gap-3">
            {(
              [
                [
                  <Database />,
                  ja ? "接続ノードのみ" : "Connected node only",
                  ja
                    ? "現在接続しているノードの記録です。"
                    : "Records are from the currently connected node.",
                ],
                [
                  <LockKeyhole />,
                  ja ? "閲覧権限に基づく記録" : "Access-limited records",
                  ja
                    ? "すべての記録が閲覧できるとは限りません。"
                    : "Only records visible with your access are included.",
                ],
              ] as const
            ).map(([icon, title, description]) => (
              <li key={title} className="flex gap-2.5">
                <span
                  aria-hidden
                  className="mt-0.5 text-faint [&_svg]:size-3.5"
                >
                  {icon}
                </span>
                <div className="grid gap-0.5">
                  <h3 className="text-[13px] font-medium text-foreground">
                    {title}
                  </h3>
                  <p className="text-xs text-muted-foreground">{description}</p>
                </div>
              </li>
            ))}
          </ul>
        </InspectorSection>
      )}
    </>
  );
}

const eventKey = (item: AuditPage["items"][number]) =>
  JSON.stringify([item.source, item.kind, item.at, item.actor, item.details]);

/** Audit tab: dense table with filters and paging, plus the event inspector. */
export function TrustAudit({
  page,
  error,
  locale,
  offset,
  tenantControl,
  onOffset,
  inspection,
  version,
}: {
  page: AuditPage | null;
  error: string;
  locale: string;
  offset: number;
  tenantControl: ReactNode;
  onOffset: (offset: number) => void;
  inspection: Inspection | null;
  version: string;
}) {
  const ja = locale === "ja-JP";
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [source, setSource] = useState("");
  const selected = page?.items.find((item) => eventKey(item) === selectedKey);
  const sources = [...new Set(page?.items.map((item) => item.source) ?? [])];
  const needle = query.trim().toLocaleLowerCase();
  const rows = (page?.items ?? []).filter(
    (item) =>
      (!source || item.source === source) &&
      (!needle ||
        JSON.stringify([item.kind, item.source, item.actor, item.details])
          .toLocaleLowerCase()
          .includes(needle)),
  );
  return (
    <>
      <MainColumn wide>
        <Section
          title={ja ? "監査履歴" : "Audit trail"}
          description={
            ja
              ? "このバージョンの閲覧可能な操作履歴を時系列で確認します。"
              : "Chronological record of visible activity for this agent version."
          }
        >
          <div className="flex flex-wrap items-end gap-3">
            {tenantControl}
            <div className="w-full sm:w-64">
              <Field label={ja ? "イベントを絞り込む" : "Filter events"}>
                <Input
                  type="search"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </Field>
            </div>
            <div className="w-full sm:w-48">
              <Field label={ja ? "取得元" : "Source"}>
                <NativeSelect
                  value={source}
                  onChange={(event) => setSource(event.target.value)}
                >
                  <option value="">{ja ? "すべて" : "All sources"}</option>
                  {sources.map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
            </div>
          </div>
          {page && (
            <p className="font-mono text-[11px] text-faint">
              {page.source_boundary} ·{" "}
              {new Date(page.observed_at).toLocaleString(locale)}
            </p>
          )}
          {rows.length ? (
            <div className="rounded-lg border border-border bg-surface">
              <Table aria-label={ja ? "監査履歴" : "Audit trail"}>
                <TableHeader>
                  <TableRow>
                    <TableHead>{ja ? "日時" : "Recorded at"}</TableHead>
                    <TableHead>{ja ? "イベント" : "Event"}</TableHead>
                    <TableHead>{ja ? "取得元" : "Source"}</TableHead>
                    <TableHead>{ja ? "操作主体" : "Actor"}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((item, index) => {
                    const key = eventKey(item);
                    const active = selectedKey === key;
                    return (
                      <TableRow
                        key={`${key}:${index}`}
                        data-state={active ? "selected" : undefined}
                        className="cursor-pointer data-[state=selected]:shadow-[inset_2px_0_0_var(--brand-mark)]"
                        onClick={() => setSelectedKey(key)}
                      >
                        <TableCell className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                          <time dateTime={item.at}>
                            {new Date(item.at).toLocaleString(locale)}
                          </time>
                        </TableCell>
                        <TableCell>
                          <button
                            type="button"
                            aria-pressed={active}
                            className="font-mono text-xs text-foreground underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                            onClick={(event) => {
                              event.stopPropagation();
                              setSelectedKey(key);
                            }}
                          >
                            {item.kind}
                          </button>
                        </TableCell>
                        <TableCell className="text-xs text-muted-foreground">
                          {item.source}
                        </TableCell>
                        <TableCell className="text-xs">
                          {item.actor ?? "—"}
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
          ) : (
            <EmptyState
              icon={<History />}
              title={
                error
                  ? ja
                    ? "監査記録を取得できません"
                    : "Unable to load audit records"
                  : page
                    ? page.items.length
                      ? ja
                        ? "条件に一致する記録はありません"
                        : "No events match these filters"
                      : ja
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
            </EmptyState>
          )}
          <div className="flex items-center gap-3">
            <Button
              variant="outline"
              size="sm"
              type="button"
              disabled={offset === 0}
              onClick={() => {
                setSelectedKey(null);
                onOffset(0);
              }}
            >
              {ja ? "最新に戻る" : "Back to latest"}
            </Button>
            <span className="font-mono text-xs tabular text-muted-foreground">
              {page
                ? page.items.length
                  ? `${offset + 1}–${offset + page.items.length}`
                  : "0"
                : "—"}
            </span>
            <Button
              variant="outline"
              size="sm"
              type="button"
              disabled={page?.next_offset == null}
              onClick={() => {
                if (page?.next_offset != null) {
                  setSelectedKey(null);
                  onOffset(page.next_offset);
                }
              }}
            >
              {ja ? "次の50件" : "Next 50"}
            </Button>
          </div>
        </Section>
      </MainColumn>
      <Inspector label={ja ? "イベントの詳細" : "Event details"}>
        <div className="trust-event-details">
          <InspectorSection title={ja ? "イベントの詳細" : "Event details"}>
            {selected ? (
              <>
                <p className="font-mono text-[13px] text-foreground">
                  {selected.kind}
                </p>
                <Facts
                  items={[
                    [ja ? "取得元" : "Source", selected.source],
                    [ja ? "操作主体" : "Actor", selected.actor ?? "—"],
                    [
                      ja ? "日時" : "Recorded at",
                      <span className="font-mono text-xs tabular">
                        {new Date(selected.at).toLocaleString(locale)}
                      </span>,
                    ],
                  ]}
                />
                <JsonView value={selected.details} />
              </>
            ) : (
              <EmptyState
                icon={<FileText />}
                title={
                  ja ? "イベントが選択されていません" : "No event selected"
                }
              >
                {ja
                  ? "監査履歴から記録を選択すると、詳細が表示されます。"
                  : "Choose an audit record from the timeline to view its details here."}
              </EmptyState>
            )}
          </InspectorSection>
        </div>
        <TrustSummary
          inspection={inspection}
          version={version}
          locale={locale}
          audit
        />
      </Inspector>
    </>
  );
}
