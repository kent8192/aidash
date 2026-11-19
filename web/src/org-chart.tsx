import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Alert, Hint, Loading } from "./components/patterns";
import { apiFetch } from "./transport";
import {
  flowStopPath,
  hiddenCount,
  nodeFlows,
  orgChartPath,
  spendText,
  stopSummary,
  type ChartNode,
  type FlowStopped,
  type NodeCreator,
  type OrgChart,
} from "./org-chart-model";
import { Panel, useI18n } from "./ui";

function errorText(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

/** Stops everything the flow of `rootTask` made, after the person confirms:
 *  its open Runs are cancelled, its Subnodes end and its Duties pause; the
 *  Nodes it created stay (#1012). */
export function FlowStopButton({
  tenant,
  rootTask,
  onStopped,
}: {
  tenant: string;
  rootTask: string;
  onStopped?: (result: FlowStopped) => void;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<FlowStopped | null>(null);
  const summary = result ? stopSummary(result) : null;
  return (
    <div className="grid min-w-0 gap-2">
      <div>
        <Button
          type="button"
          variant="destructive"
          disabled={busy}
          onClick={() => {
            if (!window.confirm(t("flowStopConfirm"))) return;
            setBusy(true);
            setError("");
            void apiFetch<FlowStopped>(flowStopPath(tenant, rootTask), {
              method: "POST",
            })
              .then((stopped) => {
                setResult(stopped);
                onStopped?.(stopped);
              })
              .catch((reason) => setError(errorText(reason)))
              .finally(() => setBusy(false));
          }}
        >
          {t("flowStop")}
        </Button>
      </div>
      {error && <Alert>{error}</Alert>}
      {summary && result && (
        <Hint role="status">
          {t("flowStopped")
            .replace("{stopped}", String(summary.stopped))
            .replace("{already}", String(summary.already))
            .replace("{failed}", String(summary.failed))}
          {result.nodes.length > 0 &&
            ` ${t("flowStoppedNodes").replace(
              "{nodes}",
              result.nodes.map((node) => node.label).join(", "),
            )}`}
        </Hint>
      )}
    </div>
  );
}

function CreatorText({ creator }: { creator: NodeCreator }) {
  const { t } = useI18n();
  switch (creator.kind) {
    case "agent":
      return (
        <>
          {t("orgChartCreatedByAgent").replace(
            "{person}",
            creator.root_subject,
          )}{" "}
          <span className="text-faint [overflow-wrap:anywhere]">
            ({t("orgChartCreatingTask")} {creator.task_id})
          </span>
        </>
      );
    case "person":
      return <>{t("orgChartCreatedByPerson")}</>;
    case "deployment":
      return <>{t("orgChartCreatedWithTenant")}</>;
  }
}

function NodeBranch({
  tenant,
  node,
  refresh,
}: {
  tenant: string;
  node: ChartNode;
  refresh: () => void;
}) {
  const { t } = useI18n();
  const hidden = hiddenCount(node);
  return (
    <li className="grid min-w-0 gap-2 rounded border p-3">
      <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2">
        <strong className="min-w-0 [overflow-wrap:anywhere]">
          {node.label}
          {node.default && ` · ${t("orgChartDefault")}`}
        </strong>
        {node.spend && (
          <span className="text-xs text-muted-foreground">
            {t("orgChartSpend")} {spendText(node.spend)} {t("budgetCredits")}
          </span>
        )}
      </div>
      <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
        {node.name} · <CreatorText creator={node.created_by} />
      </p>
      {node.duties.length > 0 && (
        <div className="grid gap-1">
          <span className="text-xs font-semibold">{t("orgChartDuties")}</span>
          <ul className="grid gap-1 border-l pl-3">
            {node.duties.map((duty) => (
              <li
                key={duty.id}
                className="min-w-0 text-sm [overflow-wrap:anywhere]"
              >
                {duty.title} · {duty.requester} · {duty.agent.id}@
                {duty.agent.version} · {t(`orgChartDutyState_${duty.state}`)}
                {duty.spend &&
                  ` · ${spendText(duty.spend)} ${t("budgetCredits")}`}
              </li>
            ))}
          </ul>
        </div>
      )}
      {node.subnodes.length > 0 && (
        <div className="grid gap-1">
          <span className="text-xs font-semibold">{t("orgChartSubnodes")}</span>
          <ul className="grid gap-1 border-l pl-3">
            {node.subnodes.map((subnode) => (
              <li
                key={subnode.id}
                className="min-w-0 text-sm [overflow-wrap:anywhere]"
              >
                {subnode.title ?? subnode.id} · {subnode.creator_agent.id}@
                {subnode.creator_agent.version} · {subnode.root_subject} ·{" "}
                {t("orgChartMembers")}{" "}
                {subnode.members
                  .map((member) => `${member.id}@${member.version}`)
                  .join(", ")}
              </li>
            ))}
          </ul>
        </div>
      )}
      {hidden > 0 && (
        <Hint>{t("orgChartHidden").replace("{count}", String(hidden))}</Hint>
      )}
      {nodeFlows(node).map((flow) => (
        <div key={flow} className="grid gap-1 border-t pt-2">
          <span className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {t("orgChartFlow")} {flow}
          </span>
          <FlowStopButton tenant={tenant} rootTask={flow} onStopped={refresh} />
        </div>
      ))}
    </li>
  );
}

/** What each chain created, under whose authority and with what spend this
 *  month (#1012, ADR 0143). The server shows only the Nodes the person may
 *  read and the spend they may see. */
export function OrgChartPanel({ tenant }: { tenant: string }) {
  const { t } = useI18n();
  const client = useQueryClient();
  const key = ["org-chart", tenant];
  const query = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<OrgChart>(orgChartPath(tenant)),
    retry: false,
  });
  const refresh = () => void client.invalidateQueries({ queryKey: key });
  return (
    <Panel title={t("orgChart")}>
      <Hint>{t("orgChartHelp")}</Hint>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {query.data && (
        <>
          <ul className="grid gap-2">
            {query.data.nodes.map((node) => (
              <NodeBranch
                key={node.name}
                tenant={tenant}
                node={node}
                refresh={refresh}
              />
            ))}
          </ul>
          {query.data.people.length > 0 && (
            <div className="grid gap-1">
              <span className="text-xs font-semibold">
                {t("orgChartPeople")}
              </span>
              <ul className="grid gap-1">
                {query.data.people.map((person) => (
                  <li key={person.subject} className="text-sm">
                    {person.subject} · {spendText(person.spend)}{" "}
                    {t("budgetCredits")}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}
    </Panel>
  );
}
