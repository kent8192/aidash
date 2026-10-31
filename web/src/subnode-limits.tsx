import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import {
  tenantSubnodeLimits,
  tenantSubnodeLimitsUpdate,
} from "./generated/aidash";
import { Alert, Check, Hint, Loading } from "./components/patterns";
import {
  MEMBERS_PER_SUBNODE,
  SUBNODES_PER_RUN,
  subnodeLimitsProblems,
} from "./subnodes-model";
import { ApiError } from "./transport";
import { Field, Panel, useI18n } from "./ui";

/** The Tenant's Subnode limits (#660), for subjects whose policy grants
 *  `subnode.limits.update`. A 403 from the server disables the form. */
export function SubnodeLimits({ tenant }: { tenant: string }) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [forbidden, setForbidden] = useState(false);
  const key = ["subnode-limits", tenant];
  const query = useQuery({
    queryKey: key,
    queryFn: () => tenantSubnodeLimits(encodeURIComponent(tenant)),
    retry: false,
  });
  const current = !query.isError ? query.data : undefined;
  const refused = forbidden || (query.isError && isForbidden(query.error));
  return (
    <Panel title={t("subnodeLimits")}>
      <Hint>{t("subnodeLimitsHelp")}</Hint>
      {query.isPending && <Loading />}
      {query.isError && !refused && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {refused && <Alert>{t("subnodeLimitsForbidden")}</Alert>}
      {error && !refused && <Alert>{error}</Alert>}
      {saved && !error && <Hint role="status">{t("saved")}</Hint>}
      {current && (
        <form
          key={`${current.members_per_subnode}:${current.subnodes_per_run}:${current.remote_runs_may_create}`}
          className="grid min-w-0 gap-3"
          onChange={() => setSaved(false)}
          onSubmit={(event) => {
            event.preventDefault();
            if (busy || refused) return;
            const data = new FormData(event.currentTarget);
            const input = {
              members_per_subnode: Number(data.get("members_per_subnode")),
              subnodes_per_run: Number(data.get("subnodes_per_run")),
              remote_runs_may_create:
                data.get("remote_runs_may_create") === "on",
            };
            if (subnodeLimitsProblems(input).length > 0) {
              setError(t("subnodeLimitsInvalid"));
              return;
            }
            setBusy(true);
            setError("");
            void tenantSubnodeLimitsUpdate(encodeURIComponent(tenant), input)
              .then(() => setSaved(true))
              .catch((reason) => {
                if (isForbidden(reason)) setForbidden(true);
                setError(
                  reason instanceof Error ? reason.message : String(reason),
                );
              })
              .finally(() => {
                setBusy(false);
                void client.invalidateQueries({ queryKey: key });
              });
          }}
        >
          <fieldset
            disabled={busy || refused}
            className="grid min-w-0 gap-3 sm:grid-cols-2"
          >
            <Field label={t("subnodeMembersPerSubnode")}>
              <Input
                name="members_per_subnode"
                type="number"
                required
                step={1}
                min={MEMBERS_PER_SUBNODE.min}
                max={MEMBERS_PER_SUBNODE.max}
                defaultValue={current.members_per_subnode}
                className="font-mono tabular"
              />
            </Field>
            <Field label={t("subnodeSubnodesPerRun")}>
              <Input
                name="subnodes_per_run"
                type="number"
                required
                step={1}
                min={SUBNODES_PER_RUN.min}
                max={SUBNODES_PER_RUN.max}
                defaultValue={current.subnodes_per_run}
                className="font-mono tabular"
              />
            </Field>
            <Check
              name="remote_runs_may_create"
              defaultChecked={current.remote_runs_may_create}
              className="sm:col-span-2"
            >
              {t("subnodeRemoteMayCreate")}
            </Check>
          </fieldset>
          <div className="flex justify-end">
            <Button disabled={busy || refused}>{t("subnodeLimitsSave")}</Button>
          </div>
        </form>
      )}
    </Panel>
  );
}

const isForbidden = (reason: unknown) =>
  reason instanceof ApiError && reason.status === 403;
