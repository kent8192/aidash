import { useState } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Alert, Facts, Hint, HistoryRow, Notice } from "./components/patterns";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import type {
  RetentionChange,
  RevocationRecord,
  RunResearch,
  TenantRetentionView,
} from "./generated/models";
import {
  MAX_REVOCATION_REASON,
  exceedsCeiling,
  hasResearch,
  periodSeconds,
  periodUnits,
  periodWords,
  recordedActor,
  shortenedPeriods,
  splitPeriod,
  type PeriodUnit,
  type Periods,
} from "./research";
import { apiFetch } from "./transport";
import { Field, Modal, useI18n } from "./ui";

/** Audit history is read 50 records at a time, newest first. */
const PAGE = 50;

type Period = keyof Periods["document_snapshots"];

const PERIODS: Record<Period, [string, string]> = {
  after_run_seconds: ["Kept after the Run ends", "Run の終了後に保持する期間"],
  max_age_seconds: ["Kept at most after retrieval", "取得後に保持する最長期間"],
};

const UNITS: Record<PeriodUnit, [string, string]> = {
  days: ["days", "日"],
  hours: ["hours", "時間"],
  minutes: ["minutes", "分"],
  seconds: ["seconds", "秒"],
};

function when(value: string, ja: boolean) {
  return new Date(value).toLocaleString(ja ? "ja-JP" : "en-US");
}

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** One Tenant's audit history, read page by page until a short page. */
function useHistory<T>(key: unknown[], path: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: key,
    initialPageParam: 0,
    queryFn: ({ pageParam, signal }) =>
      apiFetch<T[]>(`${path}?offset=${pageParam}&limit=${PAGE}`, { signal }),
    getNextPageParam: (last, pages) =>
      last.length === PAGE ? pages.length * PAGE : undefined,
    enabled,
    retry: false,
  });
}

/**
 * A Run's Evidence Revocation as its reader may see it (ADR 0046): that and
 * when it was revoked, who revoked it and why for the records' audience, and
 * the revoke action for those permitted it, offered only once the Run has
 * done research.
 */
export function RunResearchEvidence({
  run,
  deploymentAdministrator,
}: {
  run: string;
  deploymentAdministrator: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [revoking, setRevoking] = useState(false);
  const research = useQuery({
    queryKey: ["run-research", run],
    queryFn: ({ signal }) =>
      apiFetch<RunResearch>(`/api/runs/${run}/research`, { signal }),
    refetchInterval: 5000,
    retry: false,
  });
  const revoked = research.data?.revoked_at ?? null;
  // The current flag gates the facts too: a disabled query keeps the record
  // it cached while the reader still had the permission.
  const readable = !!revoked && !!research.data?.can_read_revocation;
  const record = useQuery({
    queryKey: ["run-research-revocation", run],
    queryFn: ({ signal }) =>
      apiFetch<RevocationRecord>(`/api/runs/${run}/research-revocation`, {
        signal,
      }),
    enabled: readable,
    retry: false,
  });
  const data = research.data;
  if (!data) return null;
  const offered = !revoked && data.can_revoke && hasResearch(data);
  if (!revoked && !offered) return null;
  const title = "Research evidence";
  return (
    <section
      aria-label={title}
      className="grid min-w-0 gap-2 border-t border-border pt-4"
    >
      <h3 className="text-[13px] font-semibold">{title}</h3>
      {revoked ? (
        <Notice role="status">
          {ja
            ? `この Run の Research evidence は ${when(revoked, ja)} に取り消されました。調査は終了しており、その引用は利用不可と表示されます。`
            : `This Run's research evidence was revoked on ${when(revoked, ja)}. Its research is closed, and citations of it show as unavailable.`}
        </Notice>
      ) : (
        <div>
          <Button
            type="button"
            variant="destructive"
            onClick={() => setRevoking(true)}
          >
            {ja ? "Research evidence を取り消す…" : "Revoke research evidence…"}
          </Button>
        </div>
      )}
      {readable && record.data && (
        <RevocationFacts
          record={record.data}
          deploymentAdministrator={deploymentAdministrator}
        />
      )}
      {readable && record.isError && <Alert>{message(record.error)}</Alert>}
      {revoking && <RevokeDialog run={run} close={() => setRevoking(false)} />}
    </section>
  );
}

function RevocationFacts({
  record,
  deploymentAdministrator,
  withRun = false,
}: {
  record: RevocationRecord;
  deploymentAdministrator: boolean;
  withRun?: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const removed = record.removed;
  return (
    <Facts
      items={[
        ...(withRun ? [["Run", record.run_id, true] as const] : ([] as const)),
        [
          ja ? "取り消した人" : "Revoked by",
          recordedActor(record.revoked_by, deploymentAdministrator, ja),
        ],
        [ja ? "日時" : "Revoked at", when(record.revoked_at, ja)],
        [ja ? "理由" : "Reason", record.reason],
        [
          ja ? "削除したコピー" : "Removed copies",
          ja
            ? `Source ${removed.sources} · スナップショット ${removed.snapshots} · 断片 ${removed.fragments} · 試行 ${removed.attempts} · 呼び出し ${removed.invocations}`
            : `${removed.sources} sources · ${removed.snapshots} snapshots · ${removed.fragments} fragments · ${removed.attempts} attempts · ${removed.invocations} invocations`,
        ],
        ...(removed.snapshot_digests.length > 0
          ? [
              [
                ja ? "スナップショットのダイジェスト" : "Snapshot digests",
                removed.snapshot_digests.join("\n"),
                true,
              ] as const,
            ]
          : []),
      ]}
    />
  );
}

/**
 * Revoke all of one Run's research evidence. Irreversible, so it states what
 * happens and requires a reason that is kept as audit history. `tenant`
 * guards a Run ID entered by hand: a Run of another Tenant is not revoked.
 */
function RevokeDialog({
  run,
  tenant,
  close,
}: {
  run: string;
  tenant?: string;
  close: () => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const usable = reason.trim().length > 0;
  return (
    <Modal
      title={
        ja
          ? "この Run の Research evidence を取り消す"
          : "Revoke this Run's research evidence"
      }
      close={close}
    >
      <form
        className="grid min-w-0 gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          if (!usable || busy) return;
          setBusy(true);
          setError("");
          void apiFetch<RevocationRecord>(
            `/api/runs/${encodeURIComponent(run)}/research-revocation`,
            {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({
                reason,
                ...(tenant ? { tenant } : {}),
              }),
            },
          )
            .then(async (record) => {
              client.setQueryData(["run-research-revocation", run], record);
              await Promise.all([
                client.invalidateQueries({ queryKey: ["run-research", run] }),
                client.invalidateQueries({
                  queryKey: ["research-revocations"],
                }),
              ]);
              close();
            })
            .catch((failure: unknown) => setError(message(failure)))
            .finally(() => setBusy(false));
        }}
      >
        {tenant && (
          <Facts
            items={[
              [ja ? "Tenant" : "Tenant", tenant, true],
              ["Run", run, true],
            ]}
          />
        )}
        <Notice tone="warning">
          <p>
            {ja
              ? "取り消しは元に戻せません。この Run が保存したページ本文、断片、調査結果のコピーを削除します。"
              : "Revocation cannot be undone. Aidash removes this Run's stored copies of page text, passages and research results."}
          </p>
          <ul className="mt-1 list-disc pl-4">
            <li>
              {ja
                ? "承認待ちと未送信の Web 開示はクローズされます。"
                : "Pending and unsent Web disclosure approvals close."}
            </li>
            <li>
              {ja
                ? "この Run はこれ以上調査できません。"
                : "The Run can do no further research."}
            </li>
            <li>
              {ja
                ? "公開済みの回答はそのまま残り、その引用は利用不可と表示されます。"
                : "Published answers stay, and their citations show as unavailable."}
            </li>
          </ul>
        </Notice>
        <Field label={ja ? "理由（必須）" : "Reason (required)"}>
          <Textarea
            value={reason}
            required
            maxLength={MAX_REVOCATION_REASON}
            onChange={(event) => setReason(event.target.value)}
          />
        </Field>
        <Hint>
          {ja
            ? "理由は監査履歴として残ります。URL、ページの内容、個人データは書かないでください。"
            : "The reason is kept as audit history. Do not paste URLs, page content or personal data into it."}
        </Hint>
        {error && <Alert>{error}</Alert>}
        <div className="flex flex-wrap justify-end gap-2">
          <Button type="button" variant="ghost" onClick={close}>
            {ja ? "キャンセル" : "Cancel"}
          </Button>
          <Button
            type="submit"
            variant="destructive"
            disabled={!usable || busy}
          >
            {ja ? "取り消す" : "Revoke evidence"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * A Tenant's Retention Rules and Evidence Revocation records. Deployment
 * Administrators choose the Tenant; a subject sees its own.
 */
export function ResearchEvidenceGovernance({
  tenant,
  deploymentAdministrator,
}: {
  tenant: string | null;
  deploymentAdministrator: boolean;
}) {
  const { locale, t } = useI18n();
  const ja = locale === "ja-JP";
  const [chosen, setChosen] = useState("");
  const current = deploymentAdministrator ? chosen : tenant;
  return (
    <div className="grid min-w-0 gap-4">
      {deploymentAdministrator && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            setChosen(
              String(new FormData(event.currentTarget).get("tenant")).trim(),
            );
          }}
        >
          <div className="w-full min-w-0 sm:w-72">
            <Field label={t("tenant")}>
              <Input
                name="tenant"
                required
                maxLength={256}
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
              />
            </Field>
          </div>
          <Button variant="outline">{t("open")}</Button>
        </form>
      )}
      {current ? (
        <TenantResearchEvidence
          key={current}
          tenant={current}
          deploymentAdministrator={deploymentAdministrator}
        />
      ) : (
        !deploymentAdministrator && (
          <Hint role="status">
            {ja
              ? "Tenant の主体として接続すると表示されます。"
              : "Shown when you act as a subject of a Tenant."}
          </Hint>
        )
      )}
    </div>
  );
}

function TenantResearchEvidence({
  tenant,
  deploymentAdministrator,
}: {
  tenant: string;
  deploymentAdministrator: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const path = `/api/tenants/${encodeURIComponent(tenant)}`;
  const retention = useQuery({
    queryKey: ["tenant-retention", tenant],
    queryFn: ({ signal }) =>
      apiFetch<TenantRetentionView>(`${path}/retention`, { signal }),
    retry: false,
  });
  if (retention.isPending)
    return <Hint role="status">{ja ? "読み込み中…" : "Loading…"}</Hint>;
  if (retention.isError) return <Alert>{message(retention.error)}</Alert>;
  const view = retention.data;
  return (
    <div className="grid min-w-0 gap-6">
      <section
        aria-label={ja ? "スナップショットの保持期間" : "Snapshot retention"}
        className="grid min-w-0 gap-3"
      >
        <h3 className="text-[13px] font-semibold">
          {ja ? "スナップショットの保持期間" : "Snapshot retention"}
        </h3>
        <Facts
          items={(Object.keys(PERIODS) as Period[]).map((period) => [
            PERIODS[period][ja ? 1 : 0],
            ja
              ? `${periodWords(view.effective.document_snapshots[period], ja)}（Deployment の上限 ${periodWords(view.deployment.document_snapshots[period], ja)}）`
              : `${periodWords(view.effective.document_snapshots[period], ja)} (Deployment ceiling ${periodWords(view.deployment.document_snapshots[period], ja)})`,
          ])}
        />
        <Hint>
          {view.tenant_rules
            ? ja
              ? "この Tenant 独自の期間が適用されています。"
              : "This Tenant's own periods apply."
            : ja
              ? "Deployment の期間が適用されています。"
              : "The Deployment's periods apply."}{" "}
          {ja
            ? "期間の変更は、次回の毎時の掃除で、すでに保存されている内容にも適用されます。"
            : "A change applies to content already kept, at the next hourly sweep."}
        </Hint>
        {exceedsCeiling(view.tenant_rules, view.deployment) && (
          <Notice>
            {ja
              ? "この Tenant の期間は Deployment の上限を超えているため、上限が適用されています。"
              : "This Tenant's periods exceed the Deployment's ceiling, so the ceiling applies."}
          </Notice>
        )}
        {view.can_update && <RetentionEditor tenant={tenant} view={view} />}
      </section>
      {view.can_read_history && (
        <RetentionHistory
          tenant={tenant}
          deploymentAdministrator={deploymentAdministrator}
        />
      )}
      {view.can_read_revocations && (
        <RevocationRecords
          tenant={tenant}
          deploymentAdministrator={deploymentAdministrator}
        />
      )}
    </div>
  );
}

type Draft = Record<Period, { value: string; unit: PeriodUnit }>;

function draftOf(rules: Periods): Draft {
  const part = (period: Period) => {
    const { value, unit } = splitPeriod(rules.document_snapshots[period]);
    return { value: String(value), unit };
  };
  return {
    after_run_seconds: part("after_run_seconds"),
    max_age_seconds: part("max_age_seconds"),
  };
}

/**
 * Change or clear a Tenant's own periods, never beyond the Deployment's. A
 * change is reviewed before it is saved: it applies to content already kept,
 * and both periods become the Tenant's own.
 */
function RetentionEditor({
  tenant,
  view,
}: {
  tenant: string;
  view: TenantRetentionView;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [draft, setDraft] = useState<Draft>(() => draftOf(view.effective));
  // The rules under review; null clears them back to the Deployment's.
  const [review, setReview] = useState<{ rules: Periods | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const seconds = (period: Period) => {
    const value = Number(draft[period].value);
    return Number.isInteger(value) && value >= 0
      ? periodSeconds(value, draft[period].unit)
      : null;
  };
  const invalid = (period: Period) => {
    const value = seconds(period);
    return value === null
      ? ja
        ? "0 以上の整数を入力してください。"
        : "Enter a whole number of 0 or more."
      : value > view.deployment.document_snapshots[period]
        ? ja
          ? "Deployment の上限を超えています。"
          : "This is longer than the Deployment's ceiling."
        : null;
  };
  const periods = Object.keys(PERIODS) as Period[];
  const proposed = (): Periods => ({
    document_snapshots: {
      after_run_seconds: seconds("after_run_seconds")!,
      max_age_seconds: seconds("max_age_seconds")!,
    },
  });
  const save = (rules: Periods | null) => {
    setBusy(true);
    setError("");
    void apiFetch<TenantRetentionView>(
      `/api/tenants/${encodeURIComponent(tenant)}/retention`,
      {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ rules }),
      },
    )
      .then(async (saved) => {
        client.setQueryData(["tenant-retention", tenant], saved);
        await client.invalidateQueries({
          queryKey: ["tenant-retention-history", tenant],
        });
        setDraft(draftOf(saved.effective));
        setReview(null);
      })
      .catch((failure: unknown) => setError(message(failure)))
      .finally(() => setBusy(false));
  };
  if (review) {
    const next = review.rules ?? view.deployment;
    const shortened = shortenedPeriods(view.effective, next);
    return (
      <div className="grid min-w-0 gap-3">
        <h4 className="text-xs font-semibold">
          {review.rules
            ? ja
              ? "変更の確認"
              : "Review the change"
            : ja
              ? "Deployment の期間に戻す"
              : "Clear to the Deployment's periods"}
        </h4>
        <Facts
          items={periods.map((period) => [
            PERIODS[period][ja ? 1 : 0],
            `${periodWords(view.effective.document_snapshots[period], ja)} → ${periodWords(next.document_snapshots[period], ja)}`,
          ])}
        />
        {shortened.length > 0 && (
          <Notice tone="warning">
            {ja
              ? "新しい期間を過ぎた保存済みの内容は、次回の毎時の掃除で削除され、元に戻せません。"
              : "Content already kept beyond the new periods is removed at the next hourly sweep. This cannot be undone."}
          </Notice>
        )}
        {next.document_snapshots.max_age_seconds === 0 && (
          <Notice tone="warning">
            {ja
              ? "最長期間が 0 のため、実行中の Run もページを読み直すことになり、その読み取りは課金されます。"
              : "With a maximum of 0, running Runs read pages again, and each reading is charged."}
          </Notice>
        )}
        {error && <Alert>{error}</Alert>}
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            disabled={busy}
            onClick={() => save(review.rules)}
          >
            {ja ? "保存" : "Save periods"}
          </Button>
          <Button
            type="button"
            variant="ghost"
            disabled={busy}
            onClick={() => setReview(null)}
          >
            {ja ? "戻る" : "Back"}
          </Button>
        </div>
      </div>
    );
  }
  return (
    <form
      className="grid min-w-0 gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (periods.some(invalid)) return;
        setReview({ rules: proposed() });
      }}
    >
      {periods.map((period) => {
        const problem = invalid(period);
        const ceiling = view.deployment.document_snapshots[period];
        return (
          <fieldset key={period} className="grid min-w-0 gap-1.5">
            <legend className="text-xs font-medium text-muted-foreground">
              {PERIODS[period][ja ? 1 : 0]}
            </legend>
            <div className="flex flex-wrap items-center gap-2">
              <Input
                type="number"
                min={0}
                step={1}
                className="w-28"
                aria-label={PERIODS[period][ja ? 1 : 0]}
                aria-invalid={!!problem}
                value={draft[period].value}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    [period]: { ...draft[period], value: event.target.value },
                  })
                }
              />
              <NativeSelect
                className="w-28"
                aria-label={ja ? "単位" : "Unit"}
                value={draft[period].unit}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    [period]: {
                      ...draft[period],
                      unit: event.target.value as PeriodUnit,
                    },
                  })
                }
              >
                {/* Seconds only for a period already kept in seconds. */}
                {periodUnits(draft[period].unit === "seconds" ? 1 : 0).map(
                  (unit) => (
                    <option key={unit} value={unit}>
                      {UNITS[unit][ja ? 1 : 0]}
                    </option>
                  ),
                )}
              </NativeSelect>
              <Hint>
                {ja
                  ? `上限: ${periodWords(ceiling, ja)}`
                  : `Ceiling: ${periodWords(ceiling, ja)}`}
              </Hint>
            </div>
            {problem && <Hint className="text-destructive">{problem}</Hint>}
          </fieldset>
        );
      })}
      <Hint>
        {ja
          ? "保存すると、両方の期間がこの Tenant 独自の値になります。"
          : "Saving makes both periods this Tenant's own."}
      </Hint>
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="outline">
          {ja ? "変更を確認" : "Review change"}
        </Button>
        {view.tenant_rules && (
          <Button
            type="button"
            variant="ghost"
            onClick={() => setReview({ rules: null })}
          >
            {ja
              ? "Deployment の期間に戻す"
              : "Clear to the Deployment's periods"}
          </Button>
        )}
      </div>
    </form>
  );
}

function LoadMore({
  query,
}: {
  query: {
    hasNextPage: boolean;
    isFetchingNextPage: boolean;
    fetchNextPage: () => unknown;
  };
}) {
  const { locale } = useI18n();
  if (!query.hasNextPage) return null;
  return (
    <div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={query.isFetchingNextPage}
        onClick={() => void query.fetchNextPage()}
      >
        {locale === "ja-JP" ? "さらに読み込む" : "Load more"}
      </Button>
    </div>
  );
}

function RetentionHistory({
  tenant,
  deploymentAdministrator,
}: {
  tenant: string;
  deploymentAdministrator: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const history = useHistory<RetentionChange>(
    ["tenant-retention-history", tenant],
    `/api/tenants/${encodeURIComponent(tenant)}/retention-history`,
    true,
  );
  const changes = history.data?.pages.flat() ?? [];
  const title = ja ? "保持期間の変更履歴" : "Retention history";
  return (
    <section aria-label={title} className="grid min-w-0 gap-2">
      <h3 className="text-[13px] font-semibold">{title}</h3>
      {history.isError && <Alert>{message(history.error)}</Alert>}
      {history.isSuccess && changes.length === 0 && (
        <Hint>{ja ? "変更はまだありません。" : "No changes yet."}</Hint>
      )}
      <div className="grid min-w-0">
        {changes.map((change) => (
          <HistoryRow
            key={`${change.changed_at} ${change.changed_by}`}
            time={when(change.changed_at, ja)}
            summary={
              (change.rules
                ? ja
                  ? `${periodWords(change.rules.document_snapshots.after_run_seconds, ja)} / ${periodWords(change.rules.document_snapshots.max_age_seconds, ja)} に変更`
                  : `Set to ${periodWords(change.rules.document_snapshots.after_run_seconds, ja)} / ${periodWords(change.rules.document_snapshots.max_age_seconds, ja)}`
                : ja
                  ? "Deployment の期間に戻した"
                  : "Cleared to the Deployment's periods") +
              ` · ${recordedActor(change.changed_by, deploymentAdministrator, ja)}`
            }
          >
            <Facts
              items={[
                [
                  ja ? "変更した人" : "Changed by",
                  recordedActor(change.changed_by, deploymentAdministrator, ja),
                ],
                ...(change.rules
                  ? (Object.keys(PERIODS) as Period[]).map(
                      (period) =>
                        [
                          PERIODS[period][ja ? 1 : 0],
                          periodWords(
                            change.rules!.document_snapshots[period],
                            ja,
                          ),
                        ] as const,
                    )
                  : []),
              ]}
            />
          </HistoryRow>
        ))}
      </div>
      <LoadMore query={history} />
    </section>
  );
}

function RevocationRecords({
  tenant,
  deploymentAdministrator,
}: {
  tenant: string;
  deploymentAdministrator: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [run, setRun] = useState("");
  const records = useHistory<RevocationRecord>(
    ["research-revocations", tenant],
    `/api/tenants/${encodeURIComponent(tenant)}/research-revocations`,
    true,
  );
  const list = records.data?.pages.flat() ?? [];
  const title = ja
    ? "Research evidence の取り消し記録"
    : "Evidence revocations";
  return (
    <section aria-label={title} className="grid min-w-0 gap-2">
      <h3 className="text-[13px] font-semibold">{title}</h3>
      {deploymentAdministrator && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            setRun(String(new FormData(event.currentTarget).get("run")).trim());
          }}
        >
          <div className="w-full min-w-0 sm:w-96">
            <Field
              label={ja ? "Run ID を指定して取り消す" : "Revoke by Run ID"}
            >
              <Input
                name="run"
                required
                pattern="[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
              />
            </Field>
          </div>
          <Button variant="outline">
            {ja ? "取り消しを確認" : "Review revocation"}
          </Button>
        </form>
      )}
      {run && (
        <RevokeDialog run={run} tenant={tenant} close={() => setRun("")} />
      )}
      {records.isError && <Alert>{message(records.error)}</Alert>}
      {records.isSuccess && list.length === 0 && (
        <Hint>{ja ? "取り消しはまだありません。" : "No revocations yet."}</Hint>
      )}
      <div className="grid min-w-0">
        {list.map((record) => (
          <HistoryRow
            key={record.run_id}
            time={when(record.revoked_at, ja)}
            summary={`Run ${record.run_id.slice(0, 8)} · ${recordedActor(record.revoked_by, deploymentAdministrator, ja)}`}
          >
            <RevocationFacts
              record={record}
              deploymentAdministrator={deploymentAdministrator}
              withRun
            />
          </HistoryRow>
        ))}
      </div>
      <LoadMore query={records} />
    </section>
  );
}
