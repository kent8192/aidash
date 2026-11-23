import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
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
import { Textarea } from "./components/ui/textarea";
import { budgets, budgetsCreate, budgetsDelete } from "./generated/aidash";
import type {
  BudgetView,
  ModelCallUsageByPurpose,
  RequestBudgetDefinition as BudgetDefinition,
} from "./generated/models";
import { Alert, Hint, Loading } from "./components/patterns";
import {
  budgetProblems,
  creditsToNanocredits,
  formatCredits,
  parseRecipients,
} from "./budgets-model";
import { Field, Panel, useI18n } from "./ui";

type ScopeKind = "tenant" | "node" | "duty" | "person";

function scopeOf(kind: ScopeKind, key: string): BudgetDefinition["scope"] {
  if (kind === "node") return { kind: "node", node: key };
  if (kind === "duty") return { kind: "duty", duty_id: key };
  if (kind === "person") return { kind: "person", subject: key };
  return { kind: "tenant" };
}

function scopeLabel(scope: BudgetView["scope"]): string {
  switch (scope.kind) {
    case "tenant":
      return "Tenant";
    case "node":
      return `Node ${scope.node}`;
    case "duty":
      return `Duty ${scope.duty_id}`;
    case "person":
      return scope.subject;
  }
}

/** The Budgets this person may set, with this window's spend (#1012). The
 *  server lists only what the person may change and refuses the rest. */
export function BudgetSettings({
  tenant,
  subject,
  modelCallUsage,
}: {
  tenant: string;
  subject: string | null;
  modelCallUsage?: ModelCallUsageByPurpose[] | null;
}) {
  const { t } = useI18n();
  const client = useQueryClient();
  const key = ["budgets", tenant];
  const query = useQuery({
    queryKey: key,
    queryFn: () => budgets(encodeURIComponent(tenant)),
    retry: false,
  });
  const [kind, setKind] = useState<ScopeKind>("person");
  const [scopeKey, setScopeKey] = useState(subject ?? "");
  const [period, setPeriod] = useState<"monthly" | "total">("monthly");
  const [soft, setSoft] = useState("");
  const [hard, setHard] = useState("");
  const [onHard, setOnHard] = useState<"stop" | "sleep">("stop");
  const [recipients, setRecipients] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const refresh = () => void client.invalidateQueries({ queryKey: key });
  const remove = (budget: BudgetView) => {
    setBusy(true);
    setError("");
    void budgetsDelete(encodeURIComponent(tenant), budget.id)
      .catch((reason) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      )
      .finally(() => {
        setBusy(false);
        refresh();
      });
  };
  return (
    <Panel title={t("budgets")}>
      <Hint>{t("budgetsHelp")}</Hint>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {error && <Alert>{error}</Alert>}
      {modelCallUsage && modelCallUsage.length > 0 && (
        <ModelCallUsage rows={modelCallUsage} />
      )}
      {query.data && query.data.length > 0 && (
        <ul className="grid gap-2">
          {query.data.map((budget) => (
            <li
              key={budget.id}
              className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded border p-2"
            >
              <span className="min-w-0 break-words">
                {scopeLabel(budget.scope)} ·{" "}
                {t(`budgetPeriod_${budget.period}`)}
                {" · "}
                {formatCredits(budget.spent_nanocredits)}
                {budget.hard_limit_nanocredits != null &&
                  ` / ${formatCredits(budget.hard_limit_nanocredits)}`}{" "}
                {t("budgetCredits")} · {t(`budgetStanding_${budget.standing}`)}
                {budget.unpriced_calls > 0 &&
                  ` · ${t("budgetUnpriced").replace("{count}", String(budget.unpriced_calls))}`}
              </span>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => remove(budget)}
              >
                {t("delete")}
              </Button>
            </li>
          ))}
        </ul>
      )}
      <form
        className="grid min-w-0 gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          const problems = budgetProblems({ soft, hard, recipients });
          if (problems.length > 0) {
            setError(t(`budgetProblem_${problems[0]}`));
            return;
          }
          const definition: BudgetDefinition = {
            scope: scopeOf(kind, scopeKey.trim()),
            period,
            soft_limit_nanocredits: creditsToNanocredits(soft) ?? null,
            hard_limit_nanocredits: creditsToNanocredits(hard) ?? null,
            on_hard_limit: onHard,
            recipients: parseRecipients(recipients),
          };
          setBusy(true);
          setError("");
          void budgetsCreate(encodeURIComponent(tenant), definition)
            .then(() => {
              setSoft("");
              setHard("");
            })
            .catch((reason) =>
              setError(
                reason instanceof Error ? reason.message : String(reason),
              ),
            )
            .finally(() => {
              setBusy(false);
              refresh();
            });
        }}
      >
        <Field label={t("budgetScope")}>
          <NativeSelect
            value={kind}
            onChange={(event) => {
              const next = event.target.value as ScopeKind;
              setKind(next);
              setScopeKey(next === "person" ? (subject ?? "") : "");
            }}
          >
            {(["person", "duty", "node", "tenant"] as const).map((value) => (
              <option key={value} value={value}>
                {t(`budgetScope_${value}`)}
              </option>
            ))}
          </NativeSelect>
        </Field>
        {kind !== "tenant" && (
          <Field label={t(`budgetScopeKey_${kind}`)}>
            <Input
              value={scopeKey}
              onChange={(event) => setScopeKey(event.currentTarget.value)}
            />
          </Field>
        )}
        <Field label={t("budgetPeriod")}>
          <NativeSelect
            value={period}
            onChange={(event) =>
              setPeriod(event.target.value as "monthly" | "total")
            }
          >
            <option value="monthly">{t("budgetPeriod_monthly")}</option>
            <option value="total">{t("budgetPeriod_total")}</option>
          </NativeSelect>
        </Field>
        <Field label={t("budgetSoftLimit")}>
          <Input
            inputMode="decimal"
            value={soft}
            onChange={(event) => setSoft(event.currentTarget.value)}
          />
        </Field>
        <Field label={t("budgetHardLimit")}>
          <Input
            inputMode="decimal"
            value={hard}
            onChange={(event) => setHard(event.currentTarget.value)}
          />
        </Field>
        <Field label={t("budgetOnHardLimit")}>
          <NativeSelect
            value={onHard}
            onChange={(event) =>
              setOnHard(event.target.value as "stop" | "sleep")
            }
          >
            <option value="stop">{t("budgetOnHardLimit_stop")}</option>
            <option value="sleep">{t("budgetOnHardLimit_sleep")}</option>
          </NativeSelect>
        </Field>
        <Field label={t("budgetRecipients")}>
          <Textarea
            rows={2}
            value={recipients}
            onChange={(event) => setRecipients(event.currentTarget.value)}
          />
        </Field>
        <div className="flex justify-end">
          <Button disabled={busy}>{t("budgetAdd")}</Button>
        </div>
      </form>
    </Panel>
  );
}

function ModelCallUsage({ rows }: { rows: ModelCallUsageByPurpose[] }) {
  const { locale, t } = useI18n();
  const credits = new Intl.NumberFormat(locale, { maximumFractionDigits: 9 });
  const quantities = new Intl.NumberFormat(locale);
  const reported = (value: number | null) =>
    value === null ? t("modelCallUnreported") : quantities.format(value);

  return (
    <Table>
      <caption className="caption-top px-3 py-2 text-left font-medium">
        {t("modelCallUsage")}
      </caption>
      <TableHeader>
        <TableRow>
          <TableHead>{t("modelCallPurpose")}</TableHead>
          <TableHead className="text-right">{t("modelCallCount")}</TableHead>
          <TableHead className="text-right">
            {t("modelCallInputTokens")}
          </TableHead>
          <TableHead className="text-right">
            {t("modelCallOutputTokens")}
          </TableHead>
          <TableHead className="text-right">{t("modelCallCost")}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <TableRow key={row.purpose}>
            <TableHead scope="row">
              {t(`modelCallPurpose_${row.purpose}`)}
            </TableHead>
            <TableCell className="text-right">
              {quantities.format(row.calls)}
            </TableCell>
            <TableCell className="text-right">
              {reported(row.input_tokens)}
            </TableCell>
            <TableCell className="text-right">
              {reported(row.output_tokens)}
            </TableCell>
            <TableCell className="text-right">
              {row.cost_nanocredits === null
                ? t("modelCallUnreported")
                : credits.format(row.cost_nanocredits / 1e9)}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
