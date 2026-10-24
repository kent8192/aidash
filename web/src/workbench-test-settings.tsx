import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import {
  workbenchPutTestProfile,
  workbenchSetTestLimits,
  workbenchTenantTestLimits,
  workbenchTenantTestProfiles,
} from "./generated/aidash";
import type { TestLimitCeilings, TestProfile } from "./generated/models";
import { ApiError } from "./transport";
import { Field, Panel, useI18n } from "./ui";
import { Alert, Check, Hint, Loading } from "./components/patterns";

const LIMITS = [
  "max_input_bytes",
  "max_output_tokens",
  "max_total_tokens",
  "max_steps",
  "max_duration_secs",
  "max_concurrent",
  "payload_days",
  "incident_evidence_days",
] as const satisfies readonly (keyof TestLimitCeilings)[];

const MINIMUMS: TestLimitCeilings = {
  max_input_bytes: 1024,
  max_output_tokens: 64,
  max_total_tokens: 256,
  max_steps: 1,
  max_duration_secs: 5,
  max_concurrent: 1,
  payload_days: 1,
  incident_evidence_days: 1,
};

/** The Tenant's own Workbench Test Limits and Test Profiles (#361), for
 *  subjects whose Tenant's policy grants the matching actions. */
export function WorkbenchTestSettings({
  tenant,
  can,
}: {
  tenant: string;
  can: (action: string) => boolean;
}) {
  return (
    <>
      {can("workbench.test_limits.update") && <TestLimits tenant={tenant} />}
      {can("workbench.test_profile.update") && (
        <TestProfiles key={tenant} tenant={tenant} />
      )}
    </>
  );
}

function TestLimits({ tenant }: { tenant: string }) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const query = useQuery({
    queryKey: ["workbench-test-limits", tenant],
    queryFn: () => workbenchTenantTestLimits(encodeURIComponent(tenant)),
  });
  const current = !query.isError ? query.data : undefined;
  return (
    <Panel title={t("testLimits")}>
      <Hint>{t("testLimitsHelp")}</Hint>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {error && <Alert>{error}</Alert>}
      {saved && !error && <Hint role="status">{t("saved")}</Hint>}
      {current && (
        <form
          key={current.limits.revision}
          className="grid min-w-0 gap-3"
          onChange={() => setSaved(false)}
          onSubmit={(event) => {
            event.preventDefault();
            if (busy) return;
            const data = new FormData(event.currentTarget);
            setBusy(true);
            setError("");
            const values = Object.fromEntries(
              LIMITS.map((field) => [field, Number(data.get(field))]),
            ) as Record<(typeof LIMITS)[number], number>;
            void workbenchSetTestLimits(encodeURIComponent(tenant), {
              ...values,
              tenant,
              revision: current.limits.revision,
            })
              .then(() => setSaved(true))
              .catch((reason) =>
                setError(
                  reason instanceof ApiError && reason.status === 409
                    ? t("testLimitsChanged")
                    : reason instanceof Error
                      ? reason.message
                      : String(reason),
                ),
              )
              .finally(() => {
                setBusy(false);
                void client.invalidateQueries({
                  queryKey: ["workbench-test-limits", tenant],
                });
              });
          }}
        >
          <fieldset
            disabled={busy}
            className="grid min-w-0 gap-3 sm:grid-cols-2 lg:grid-cols-4"
          >
            {LIMITS.map((field) => (
              <div key={field} className="grid min-w-0 gap-1">
                <Field label={t(`testLimit_${field}`)}>
                  <Input
                    name={field}
                    type="number"
                    required
                    min={MINIMUMS[field]}
                    max={current.ceilings[field]}
                    defaultValue={current.limits[field]}
                    className="font-mono tabular"
                  />
                </Field>
                <span className="text-[11px] text-faint">
                  {t("testLimitCeiling")}{" "}
                  <span className="font-mono tabular">
                    {current.ceilings[field]}
                  </span>
                </span>
              </div>
            ))}
          </fieldset>
          <div className="flex justify-end">
            <Button disabled={busy}>{t("testLimitsSave")}</Button>
          </div>
        </form>
      )}
    </Panel>
  );
}

const NEW_PROFILE = "";

function TestProfiles({ tenant }: { tenant: string }) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [selected, setSelected] = useState(NEW_PROFILE);
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]);
  const [savedProfile, setSavedProfile] = useState<TestProfile>();
  const after = cursors[cursors.length - 1];
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const profiles = useQuery({
    queryKey: ["workbench-test-profiles", tenant, after],
    queryFn: () =>
      workbenchTenantTestProfiles(
        encodeURIComponent(tenant),
        after ? { after } : undefined,
      ),
  });
  const rows: TestProfile[] = !profiles.isError ? (profiles.data ?? []) : [];
  const listed = rows.find((row) => row.id === selected);
  const profile = profiles.isError
    ? undefined
    : savedProfile?.id === selected &&
        (!listed || savedProfile.revision > listed.revision)
      ? savedProfile
      : listed;
  const changePage = (next: (string | undefined)[]) => {
    setCursors(next);
    setSelected(NEW_PROFILE);
    setSavedProfile(undefined);
    setError("");
  };
  return (
    <Panel title={t("testProfiles")}>
      <Hint>{t("testProfilesHelp")}</Hint>
      {profiles.isPending && <Loading />}
      {profiles.isError && (
        <Alert retry={() => void profiles.refetch()}>
          {profiles.error.message}
        </Alert>
      )}
      {error && <Alert>{error}</Alert>}
      <Field label={t("testProfileId")}>
        <NativeSelect
          value={selected}
          disabled={busy || profiles.isPending}
          onChange={(event) => {
            setSelected(event.target.value);
            setError("");
          }}
          className="font-mono"
        >
          <option value={NEW_PROFILE}>{t("testProfileNew")}</option>
          {profile && !listed && (
            <option value={profile.id}>{profile.id}</option>
          )}
          {rows.map((row) => (
            <option key={row.id} value={row.id}>
              {row.id}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <div className="flex justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={busy || profiles.isPending || cursors.length === 1}
          onClick={() => changePage(cursors.slice(0, -1))}
        >
          {t("previous")}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={busy || profiles.isPending || rows.length < 100}
          onClick={() => changePage([...cursors, rows[rows.length - 1].id])}
        >
          {t("next")}
        </Button>
      </div>
      <form
        key={`${selected}:${profile?.revision ?? 0}`}
        className="grid min-w-0 gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          const data = new FormData(event.currentTarget);
          const id = profile ? profile.id : String(data.get("id")).trim();
          let rules;
          try {
            rules = JSON.parse(String(data.get("rules")));
            if (!Array.isArray(rules))
              throw new Error("rules must be an array");
          } catch {
            setError(t("testProfileRulesArray"));
            return;
          }
          setBusy(true);
          setError("");
          void workbenchPutTestProfile(
            encodeURIComponent(tenant),
            encodeURIComponent(id),
            {
              expected_revision: profile?.revision ?? 0,
              enabled: data.get("enabled") === "on",
              rules,
            },
          )
            .then((saved) => {
              setSavedProfile(saved);
              setSelected(id);
            })
            .catch((reason) =>
              setError(
                reason instanceof ApiError && reason.status === 409
                  ? t("authConflict")
                  : reason instanceof Error
                    ? reason.message
                    : String(reason),
              ),
            )
            .finally(() => {
              setBusy(false);
              void client.invalidateQueries({
                queryKey: ["workbench-test-profiles", tenant],
              });
            });
        }}
      >
        <fieldset
          disabled={busy || profiles.isPending || profiles.isError}
          className="grid min-w-0 gap-3"
        >
          {!profile && (
            <Field label={t("testProfileId")}>
              <Input
                name="id"
                required
                maxLength={100}
                autoComplete="off"
                className="font-mono"
              />
            </Field>
          )}
          <Field label={t("testProfileRules")}>
            <Textarea
              name="rules"
              rows={8}
              spellCheck={false}
              className="font-mono text-xs"
              defaultValue={JSON.stringify(profile?.rules ?? [], null, 2)}
            />
          </Field>
          <Check name="enabled" defaultChecked={profile?.enabled ?? true}>
            {t("enabled")}
          </Check>
        </fieldset>
        <div className="flex justify-end">
          <Button disabled={busy}>{t("testProfileSave")}</Button>
        </div>
      </form>
    </Panel>
  );
}
