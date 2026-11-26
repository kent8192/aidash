import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import {
  tenantExecutionSettings,
  tenantExecutionSettingsUpdate,
  tenantGoalJudgeModel,
  tenantGoalJudgeModelUpdate,
} from "./generated/aidash";
import { Alert, Hint, Loading } from "./components/patterns";
import { ApiError } from "./transport";
import { Field, Panel, useI18n } from "./ui";

/** The Tenant's Lease Recovery limit (#1009): how many times in a row a step
 *  may be taken over from a stopped worker before it fails. Every member reads
 *  it; `canUpdate` comes from `tenant.update`. */
export function ExecutionSettings({
  tenant,
  canUpdate,
}: {
  tenant: string;
  canUpdate: boolean;
}) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const key = ["execution-settings", tenant];
  const query = useQuery({
    queryKey: key,
    queryFn: () => tenantExecutionSettings(encodeURIComponent(tenant)),
    retry: false,
  });
  const current = !query.isError ? query.data : undefined;
  return (
    <Panel title={t("executionSettings")}>
      <Hint>{t("leaseRecoveryLimitHelp")}</Hint>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {error && <Alert>{error}</Alert>}
      {saved && !error && <Hint role="status">{t("saved")}</Hint>}
      {current && (
        <form
          key={current.revision}
          className="grid min-w-0 gap-3 sm:grid-cols-[minmax(0,12rem)_auto] sm:items-end"
          onSubmit={(event) => {
            event.preventDefault();
            if (busy || !canUpdate) return;
            const limit = Number(
              new FormData(event.currentTarget).get("lease_recovery_limit"),
            );
            setBusy(true);
            setError("");
            setSaved(false);
            void tenantExecutionSettingsUpdate(encodeURIComponent(tenant), {
              lease_recovery_limit: limit,
              expected_revision: current.revision,
            })
              .then(() => setSaved(true))
              .catch((reason) => {
                setError(
                  reason instanceof ApiError && reason.status === 409
                    ? t("instructionsConflict")
                    : reason instanceof Error
                      ? reason.message
                      : String(reason),
                );
              })
              .finally(() => {
                setBusy(false);
                void client.invalidateQueries({ queryKey: key });
              });
          }}
        >
          <Field label={t("leaseRecoveryLimit")}>
            <Input
              type="number"
              name="lease_recovery_limit"
              required
              min={1}
              max={20}
              className="font-mono tabular"
              defaultValue={current.lease_recovery_limit}
              readOnly={!canUpdate}
              disabled={busy}
            />
          </Field>
          {canUpdate && (
            <div className="flex justify-end">
              <Button disabled={busy}>{t("save")}</Button>
            </div>
          )}
        </form>
      )}
      <GoalJudgeSettings tenant={tenant} canUpdate={canUpdate} />
    </Panel>
  );
}

function GoalJudgeSettings({
  tenant,
  canUpdate,
}: {
  tenant: string;
  canUpdate: boolean;
}) {
  const { locale, t } = useI18n();
  const ja = locale === "ja-JP";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const path = encodeURIComponent(tenant);
  const settings = useQuery({
    queryKey: ["goal-judge-model", tenant],
    queryFn: () => tenantGoalJudgeModel(path),
    retry: false,
  });
  const model = settings.data?.model;
  if (settings.isPending) return <Loading />;
  if (settings.isError)
    return (
      <Alert retry={() => void settings.refetch()}>
        {settings.error.message}
      </Alert>
    );
  return (
    <form
      key={model ? `${model.id}@${model.version}` : "default"}
      className="mt-4 grid gap-3 border-t border-border pt-4"
      onSubmit={(event) => {
        event.preventDefault();
        if (!canUpdate || busy) return;
        const values = new FormData(event.currentTarget);
        const id = String(values.get("model") ?? "").trim();
        const version = String(values.get("version") ?? "").trim();
        if (Boolean(id) !== Boolean(version)) {
          setError(
            ja
              ? "モデル ID とバージョンを両方入力してください。"
              : "Enter both a model ID and a version.",
          );
          return;
        }
        setBusy(true);
        setError("");
        void tenantGoalJudgeModelUpdate(path, {
          model: id && version ? { id, version } : null,
        })
          .then(() => settings.refetch())
          .catch((reason: unknown) =>
            setError(reason instanceof Error ? reason.message : String(reason)),
          )
          .finally(() => setBusy(false));
      }}
    >
      <Hint>
        {ja
          ? "未設定の場合、タスクを実行する Agent と同じモデルで判定します。"
          : "Leaving both fields empty uses the working Agent’s model for Goal judgments."}
      </Hint>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={ja ? "ゴール判定モデル ID" : "Goal judge model ID"}>
          <Input
            name="model"
            defaultValue={model?.id ?? ""}
            readOnly={!canUpdate}
            disabled={busy}
          />
        </Field>
        <Field label={ja ? "モデルのバージョン" : "Model version"}>
          <Input
            name="version"
            defaultValue={model?.version ?? ""}
            readOnly={!canUpdate}
            disabled={busy}
          />
        </Field>
      </div>
      {error && <Alert>{error}</Alert>}
      {canUpdate && (
        <Button className="justify-self-end" disabled={busy}>
          {t("save")}
        </Button>
      )}
    </form>
  );
}
