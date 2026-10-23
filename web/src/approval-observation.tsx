import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Disclosure, Facts, Hint, Loading } from "./components/patterns";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Field, Panel, useI18n } from "./ui";
import { apiFetch } from "./transport";
import { useNodes } from "./nodes";
import { CatalogApprovalLink } from "./approval-decider";
import { DeciderRecipients, useRegistryVersions } from "./catalog-approval";
import { deciderRecipients } from "./decider";
import {
  observationProbability,
  observationRange,
  observationRate,
  type ObservationKind,
  type ObservationMetrics,
  type ObservationProjection,
  type ObservationReport,
  type ObserverSetting,
} from "./approval-observation-model";

const kinds: Record<ObservationKind, [string, string]> = {
  tool_call: ["Tool call", "ツール呼び出し"],
  outbound: ["Outbound", "外部接続"],
  web_search: ["Web search", "Web 検索"],
  page_reading: ["Page reading", "ページ読み取り"],
  generation: ["Generation", "生成"],
};
const categories: Record<string, [string, string]> = {
  data_exfiltration: ["Data exfiltration", "データ持ち出し"],
  destructive: ["Destructive", "破壊的操作"],
  beyond_request: ["Beyond request", "依頼の範囲外"],
  injected: ["Injected instruction", "注入された指示"],
  boundary: ["Human boundary", "人が指定した境界"],
};
const unavailable: Record<string, [string, string]> = {
  missing_state: ["Original state unavailable", "元の状態を復元できません"],
  definition_unapproved: [
    "Exact definition unapproved",
    "正確な定義が未承認です",
  ],
  credential_unavailable: [
    "Credentials unavailable",
    "認証情報が利用できません",
  ],
  calls_exhausted: ["Call budget exhausted", "呼び出し予算を使い切りました"],
  provider_failure: [
    "Provider failure",
    "プロバイダーの呼び出しに失敗しました",
  ],
  invalid_answers: ["Invalid answers", "応答が不正です"],
  authority_revoked: [
    "Disclosure authority revoked",
    "開示権限が取り消されました",
  ],
  journal_failure: ["Evidence unavailable", "判断の根拠が利用できません"],
  state_too_large: ["State exceeds the bound", "状態が上限を超えています"],
};
const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
const key = (ref: { id: string; version: string }) =>
  `${ref.id}@${ref.version}`;
function Probability({ bits }: { bits: string }) {
  const { locale } = useI18n();
  const value = observationProbability(bits);
  return (
    <span>
      {value === null
        ? "N/A"
        : new Intl.NumberFormat(locale, { maximumFractionDigits: 4 }).format(
            value,
          )}
    </span>
  );
}

/** Authorized per-approval comparison: no state body, action arguments or private IDs. */
export function ApprovalObservation({
  tenant,
  kind,
  approval,
}: {
  tenant: string;
  kind: "capability" | "generation";
  approval: string;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const query = useQuery({
    queryKey: ["approval-observation", tenant, kind, approval],
    queryFn: ({ signal }) =>
      apiFetch<ObservationProjection | null>(
        `/api/tenants/${encodeURIComponent(tenant)}/approval-observations/${kind}/${encodeURIComponent(approval)}`,
        { signal },
      ).then((value) => {
        if (
          value !== null &&
          (!value.outcome || !value.decider || !Array.isArray(value.routes))
        )
          throw new Error("Unexpected observation projection");
        return value;
      }),
    enabled: !!tenant,
    retry: false,
    refetchInterval: (query) =>
      !query.state.data ||
      query.state.data.outcome.status === "pending" ||
      query.state.data.human_label === null
        ? 3000
        : false,
  });
  const item = query.isError ? null : query.data;
  return (
    <Disclosure
      summary={
        ja ? "Shadow の観測と人の判断" : "Shadow observation and human decision"
      }
    >
      <Hint>
        {ja
          ? "観測は承認を変更せず、人の応答を待たせません。明示的な許可・拒否だけを比較します。"
          : "Observation never changes approval or delays a human response. Only explicit human allow or deny is compared."}
      </Hint>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert>
          {ja ? "観測を読み込めません。" : "Observation could not be loaded."}{" "}
          {query.error.message}
        </Alert>
      )}
      {query.isSuccess && item == null && (
        <Hint role="status">
          {ja
            ? "観測なし（無効、または記録なし）。人の拒否を意味しません。"
            : "No observation (disabled or missing). This is not a human denial."}
        </Hint>
      )}
      {item && (
        <>
          <Facts
            items={[
              [
                ja ? "承認の種類" : "Approval kind",
                kinds[item.kind]?.[ja ? 1 : 0] ?? item.kind,
              ],
              ["Decider", <code>{key(item.decider)}</code>],
              ...(item.decider.registry_deployment
                ? [
                    [
                      ja ? "Registry の Deployment" : "Registry deployment",
                      item.decider.registry_deployment,
                    ] as const,
                  ]
                : []),
              [
                ja ? "設定リビジョン" : "Setting revision",
                item.settings_revision,
              ],
              [
                ja ? "定義ダイジェスト" : "Definition digest",
                item.definition_digest,
              ],
              [
                ja ? "構成ダイジェスト" : "Configuration digest",
                item.configuration_digest,
              ],
              [
                ja ? "Shadow の判断" : "Shadow verdict",
                item.outcome.status === "available"
                  ? item.outcome.verdict === "allowed"
                    ? ja
                      ? "許可"
                      : "Allow"
                    : `${ja ? "ブロック" : "Block"} · ${categories[item.outcome.verdict.blocked]?.[ja ? 1 : 0] ?? item.outcome.verdict.blocked}`
                  : item.outcome.status === "pending"
                    ? ja
                      ? "観測待ち"
                      : "Pending observation"
                    : (unavailable[item.outcome.reason]?.[ja ? 1 : 0] ??
                      item.outcome.reason),
              ],
              [
                ja ? "明示的な人の判断" : "Explicit human label",
                item.human_label === "allow"
                  ? ja
                    ? "許可"
                    : "Allow"
                  : item.human_label === "deny"
                    ? ja
                      ? "拒否"
                      : "Deny"
                    : ja
                      ? "ラベルなし（未応答・自動判断など）"
                      : "Unlabelled (pending, automated or excluded)",
              ],
            ]}
          />
          {item.routes.map((route) => (
            <div key={route.route} className="grid min-w-0 gap-2">
              <Facts
                items={[
                  ["Route", `${route.route + 1} · ${route.provider_contract}`],
                  [
                    ja ? "モデル・ホスト" : "Model and hosts",
                    `${route.model} · ${route.hosts.join(", ")}`,
                  ],
                  [
                    ja ? "有効なしきい値" : "Effective threshold",
                    <Probability bits={route.effective_threshold} />,
                  ],
                ]}
              />
              <Facts
                items={Object.entries(route.answers).map(
                  ([category, probability]) => [
                    categories[category]?.[ja ? 1 : 0] ?? category,
                    <Probability bits={probability} />,
                  ],
                )}
              />
            </div>
          ))}
          <Hint>
            {ja
              ? "5つの確率は独立したカテゴリへの回答です。最大値や 1−最大値は、人が拒否・許可する校正済み確率ではありません。"
              : "The five probabilities answer independent category questions. Their maximum, or one minus the maximum, is not a calibrated probability of a human decision."}
          </Hint>
        </>
      )}
    </Disclosure>
  );
}

export function ApprovalObserverPanel({
  tenant,
  subject,
  canUpdate,
}: {
  tenant: string;
  subject: string;
  canUpdate: boolean;
}) {
  const { locale, entityName } = useI18n();
  const ja = locale === "ja-JP";
  const node = useNodes().selected?.name;
  const client = useQueryClient();
  const queryKey = ["approval-observer", tenant, node];
  const setting = useQuery({
    queryKey,
    queryFn: ({ signal }) =>
      apiFetch<ObserverSetting>(
        `/api/tenants/${encodeURIComponent(tenant)}/approval-observer?node=${encodeURIComponent(node!)}`,
        { signal },
      ).then((value) => {
        if (
          !Number.isSafeInteger(value?.revision) ||
          !(value.decider === null || value.decider?.id)
        )
          throw new Error("Unexpected observer setting");
        return value;
      }),
    enabled: !!node,
    retry: false,
  });
  const versions = useRegistryVersions({ kind: "subject", tenant, subject });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const current = setting.data;
  const choices = (versions.data ?? []).filter(
    (version) =>
      version.entry.kind === "decider" &&
      version.entry.config?.hook === "approval_observation" &&
      version.entry.config?.mode === "shadow" &&
      version.approval?.enabled === true,
  );
  const selected = current?.decider
    ? (versions.data ?? []).find(
        (version) => key(version.entry) === key(current.decider!),
      )
    : undefined;
  const recipients = selected ? deciderRecipients(selected.entry) : null;
  const choose = async (value: string) => {
    if (!current) return;
    setBusy(true);
    setError("");
    setSaved(false);
    try {
      const version = choices.find((version) => key(version.entry) === value);
      if (value && !version)
        throw new Error(
          ja
            ? "承認済みの Shadow バージョンを選択してください"
            : "Choose an approved exact Shadow version",
        );
      await apiFetch(
        `/api/tenants/${encodeURIComponent(tenant)}/approval-observer?node=${encodeURIComponent(node!)}`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            expected_revision: current.revision,
            decider: version
              ? { id: version.entry.id, version: version.entry.version }
              : null,
          }),
        },
      );
      await client.invalidateQueries({ queryKey });
      setSaved(true);
    } catch (error) {
      setError(errorText(error));
      await client.invalidateQueries({ queryKey });
    } finally {
      setBusy(false);
    }
  };
  return (
    <Panel
      title={
        ja
          ? "独立した Shadow 承認の観測"
          : "Independent Shadow approval observer"
      }
    >
      <Hint>
        {ja
          ? "Auto の強制判断とは別の Tenant 設定です。既定では無効で、追加のプロバイダー呼び出しはありません。Manual・明示的な承認依頼を含む5種類を観測します。設定変更は今後の観測だけに適用されます。"
          : "This Tenant setting is separate from Auto enforcement. Disabled by default, it makes no extra provider calls. It observes all five kinds, including Manual and explicit asks. Changes affect future observations only."}
      </Hint>
      {setting.isPending && <Loading />}
      {setting.isError && <Alert>{setting.error.message}</Alert>}
      {current && (
        <>
          <Facts
            items={[
              [
                ja ? "選択中" : "Selected",
                current.decider
                  ? key(current.decider)
                  : ja
                    ? "無効"
                    : "Disabled",
              ],
              [ja ? "設定リビジョン" : "Setting revision", current.revision],
            ]}
          />
          {canUpdate && (
            <Field
              label={
                ja
                  ? "承認済みの正確な Shadow 観測 Decider"
                  : "Approved exact Shadow observation Decider"
              }
            >
              <NativeSelect
                disabled={busy || versions.isPending}
                value={current.decider ? key(current.decider) : ""}
                onChange={(event) => void choose(event.target.value)}
              >
                <option value="">
                  {ja
                    ? "無効（追加呼び出しなし）"
                    : "Disabled (no extra calls)"}
                </option>
                {choices.map(({ entry }) => (
                  <option key={key(entry)} value={key(entry)}>
                    {entityName(entry)} · {key(entry)}
                  </option>
                ))}
                {current.decider &&
                  !choices.some(
                    ({ entry }) => key(entry) === key(current.decider!),
                  ) && (
                    <option value={key(current.decider)}>
                      {key(current.decider)} ·{" "}
                      {ja ? "現在利用不可" : "Currently unavailable"}
                    </option>
                  )}
              </NativeSelect>
            </Field>
          )}
          {current.decider && (
            <CatalogApprovalLink reference={current.decider} />
          )}
          {recipients && <DeciderRecipients recipients={recipients} />}
          {versions.isError && <Alert>{versions.error.message}</Alert>}
          {busy && <Loading />}
          {saved && (
            <Hint role="status">
              {ja
                ? "保存しました。今後の観測に適用します。"
                : "Saved for future observations."}
            </Hint>
          )}
          {error && <Alert>{error}</Alert>}
        </>
      )}
    </Panel>
  );
}

const exclusionLabels: Record<
  keyof ObservationMetrics["exclusions"],
  [string, string]
> = {
  rule: ["Rule", "ルール"],
  auto: ["Auto", "Auto"],
  bypass: ["Bypass", "Bypass"],
  expired: ["Expired", "期限切れ"],
  cancelled: ["Cancelled", "キャンセル"],
  revoked: ["Revoked", "取消済み"],
  pending: ["Pending approval", "承認待ち"],
  missing_observations: ["Missing observations", "観測なし"],
  unavailable_observations: ["Unavailable observations", "観測不可"],
  pending_observations: ["Pending observations", "観測待ち"],
};
function Metrics({ metrics }: { metrics: ObservationMetrics }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const m = metrics;
  return (
    <div className="grid min-w-0 gap-3">
      {m.total_approvals === 0 && (
        <Hint role="status">
          {ja
            ? "この期間の承認はありません。"
            : "No approvals in this interval."}
        </Hint>
      )}
      {m.total_approvals > 0 && m.comparable === 0 && (
        <Hint role="status">
          {ja
            ? "比較に必要な観測と明示的な人の判断が揃っていません。"
            : "Insufficient data: no observation paired with an explicit human decision."}
        </Hint>
      )}
      <Facts
        items={[
          [ja ? "承認総数" : "Total approvals", m.total_approvals],
          [ja ? "一致率" : "Agreement", observationRate(m.agreement, locale)],
          [
            ja
              ? "人の拒否のうち Shadow が許可"
              : "False allows among human denials",
            observationRate(m.false_allow_among_human_denials, locale),
          ],
          [
            ja
              ? "Shadow の許可のうち人が拒否"
              : "False allows among Shadow allows",
            observationRate(m.false_allow_among_shadow_allows, locale),
          ],
          [
            ja
              ? "人の許可のうち Shadow がブロック"
              : "False blocks among human approvals",
            observationRate(m.false_block_among_human_approvals, locale),
          ],
          [
            ja ? "観測の利用可能率" : "Observation availability",
            observationRate(m.availability, locale),
          ],
          [
            ja ? "人のラベルの割合" : "Explicit-label coverage",
            observationRate(m.label_coverage, locale),
          ],
          [
            ja ? "比較可能な割合" : "Comparable coverage",
            observationRate(m.comparable_coverage, locale),
          ],
        ]}
      />
      <div className="overflow-x-auto">
        <table className="w-full text-left text-xs">
          <caption className="text-left text-muted-foreground">
            {ja
              ? "Shadow と人の2×2比較（明示的な人の判断のみ）"
              : "Shadow × human 2×2 (explicit human labels only)"}
          </caption>
          <thead>
            <tr>
              <th scope="col">Shadow</th>
              <th scope="col">{ja ? "人が許可" : "Human allow"}</th>
              <th scope="col">{ja ? "人が拒否" : "Human deny"}</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <th scope="row">{ja ? "許可" : "Allow"}</th>
              <td>{m.confusion.shadow_allow_human_allow}</td>
              <td>{m.confusion.shadow_allow_human_deny}</td>
            </tr>
            <tr>
              <th scope="row">{ja ? "ブロック" : "Block"}</th>
              <td>{m.confusion.shadow_block_human_allow}</td>
              <td>{m.confusion.shadow_block_human_deny}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <Disclosure
        summary={
          ja
            ? "除外と欠測の件数（重複あり）"
            : "Exclusions and missingness (counts may overlap)"
        }
      >
        <Facts
          items={(
            Object.keys(exclusionLabels) as (keyof typeof exclusionLabels)[]
          ).map((name) => [
            exclusionLabels[name][ja ? 1 : 0],
            m.exclusions[name],
          ])}
        />
      </Disclosure>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-xs">
          <caption className="text-left text-muted-foreground">
            {ja
              ? "最大カテゴリ確率の帯別集計"
              : "Maximum-category probability bands"}
          </caption>
          <thead>
            <tr>
              <th scope="col">{ja ? "区間" : "Interval"}</th>
              <th scope="col">{ja ? "観測数" : "Observations"}</th>
              <th scope="col">{ja ? "明示ラベル数" : "Explicit labels"}</th>
              <th scope="col">
                {ja ? "人の拒否率" : "Empirical human-denial rate"}
              </th>
            </tr>
          </thead>
          <tbody>
            {m.risk_bands.map((band) => (
              <tr key={band.lower_inclusive}>
                <th scope="row">
                  [{band.lower_inclusive.toFixed(1)}, {band.upper.toFixed(1)}
                  {band.upper_inclusive ? "]" : ")"}
                </th>
                <td>{band.observations}</td>
                <td>{band.explicit_labels}</td>
                <td>
                  {observationRate(band.empirical_human_denial_rate, locale)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
export function ApprovalObservationReportPanel({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [range, setRange] = useState(() => {
    const end = new Date();
    end.setUTCDate(end.getUTCDate() + 1);
    const start = new Date(end);
    start.setUTCDate(start.getUTCDate() - 7);
    return {
      start: start.toISOString().slice(0, 10),
      end: end.toISOString().slice(0, 10),
    };
  });
  const [error, setError] = useState("");
  const query = useQuery({
    queryKey: ["approval-observation-report", tenant, range],
    queryFn: ({ signal }) => {
      const interval = observationRange(range.start, range.end);
      return apiFetch<ObservationReport>(
        `/api/tenants/${encodeURIComponent(tenant)}/approval-observations/report?${new URLSearchParams(interval)}`,
        { signal },
      ).then((value) => {
        if (!value?.metrics || !Array.isArray(value.groups))
          throw new Error("Unexpected observation report");
        return value;
      });
    },
    retry: false,
  });
  const report = query.isError ? null : query.data;
  return (
    <Panel
      title={
        ja ? "Tenant の Shadow 承認レポート" : "Tenant Shadow approval report"
      }
    >
      <Hint>
        {ja
          ? "開始を含み終了を含まない UTC 期間（最大31日）です。明示的な人のラベルだけを比較します。2値ラベルでは各カテゴリの確率は検証できず、しきい値の自動調整は行いません。"
          : "UTC interval includes start and excludes end (maximum 31 days). Comparisons use explicit human labels only. Binary labels do not validate individual category probabilities; thresholds are never tuned automatically."}
      </Hint>
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          const form = new FormData(event.currentTarget);
          const next = {
            start: String(form.get("start")),
            end: String(form.get("end")),
          };
          try {
            observationRange(next.start, next.end);
            setError("");
            setRange(next);
          } catch (error) {
            setError(
              ja
                ? "開始より後の終了日を指定し、31日以内にしてください。"
                : errorText(error),
            );
          }
        }}
      >
        <Field
          label={ja ? "開始日（UTC・含む）" : "Start date (UTC, inclusive)"}
        >
          <Input type="date" name="start" required defaultValue={range.start} />
        </Field>
        <Field
          label={ja ? "終了日（UTC・含まない）" : "End date (UTC, exclusive)"}
        >
          <Input type="date" name="end" required defaultValue={range.end} />
        </Field>
        <Button type="submit" variant="outline">
          {ja ? "レポートを表示" : "Show report"}
        </Button>
      </form>
      {error && <Alert>{error}</Alert>}
      {query.isPending && <Loading />}
      {query.isError && <Alert>{query.error.message}</Alert>}
      {report && (
        <>
          <Hint>
            {report.range.start} ≤ {ja ? "承認作成日時" : "approval creation"}{" "}
            &lt; {report.range.end}
          </Hint>
          <Metrics metrics={report.metrics} />
          <Hint>
            {ja
              ? "帯の値は5カテゴリの確率の最大値です。人の拒否確率として校正された値ではありません。少数標本から品質を断定しないでください。"
              : "Bands use the maximum of the five category probabilities, not a calibrated probability of human denial. Small samples do not establish quality."}
          </Hint>
          {report.groups.map(({ group, metrics }, index) => (
            <Disclosure
              key={index}
              summary={`${key(group.decider)} · ${kinds[group.kind]?.[ja ? 1 : 0] ?? group.kind} · ${group.model ?? "N/A"} · ${ja ? "設定" : "setting"} ${group.settings_revision}`}
            >
              <Facts
                items={[
                  ["Decider", key(group.decider)],
                  ...(group.decider.registry_deployment
                    ? [
                        [
                          ja ? "Registry の Deployment" : "Registry deployment",
                          group.decider.registry_deployment,
                        ] as const,
                      ]
                    : []),
                  [
                    ja ? "設定リビジョン" : "Setting revision",
                    group.settings_revision,
                  ],
                  [
                    ja ? "定義ダイジェスト" : "Definition digest",
                    group.definition_digest,
                  ],
                  [
                    ja ? "構成ダイジェスト" : "Configuration digest",
                    group.configuration_digest,
                  ],
                  [
                    ja ? "操作・効果" : "Operation and effect",
                    `${group.operation_class ?? "N/A"} · ${group.effect_class ?? "N/A"}`,
                  ],
                  ["Route", group.route === null ? "N/A" : group.route + 1],
                  [
                    ja
                      ? "プロバイダー・モデル・ホスト"
                      : "Provider, model and hosts",
                    `${group.provider_contract ?? "N/A"} · ${group.model ?? "N/A"} · ${group.hosts.join(", ")}`,
                  ],
                  [
                    ja ? "有効なしきい値" : "Effective threshold",
                    <Probability bits={group.effective_threshold} />,
                  ],
                ]}
              />
              <Metrics metrics={metrics} />
            </Disclosure>
          ))}
        </>
      )}
    </Panel>
  );
}
