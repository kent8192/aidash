import { Badge as StatusBadge } from "./components/ui/badge";
import { Skeleton } from "./components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { useQuery } from "@tanstack/react-query";
import { deploymentStatus } from "./generated/aidash";
import { Alert, Notice } from "./components/patterns";
import { Empty, Panel, useI18n, type StatusTone } from "./ui";

const phaseTones: Record<string, StatusTone> = {
  Running: "success",
  Succeeded: "neutral",
  Pending: "warning",
  Failed: "danger",
  Unknown: "warning",
};

const numberCell = "h-auto py-2 text-right align-top font-mono leading-5";
const tableFrame = "min-w-0 rounded-md border border-border bg-surface";

function Conditions({
  items,
}: {
  items: { kind: string; status?: string; reason: string; message: string }[];
}) {
  if (!items.length) return null;
  return (
    <ul className="mt-1 grid gap-0.5">
      {items.map((c, i) => (
        <li key={i} className="text-xs text-muted-foreground">
          <span className="font-mono text-foreground">
            {c.kind}
            {c.status ? `: ${c.status}` : ""} · {c.reason}
          </span>{" "}
          {c.message}
        </li>
      ))}
    </ul>
  );
}

export function DeploymentPage() {
  const { t } = useI18n();
  const status = useQuery({
    queryKey: ["deployment"],
    queryFn: () => deploymentStatus(),
    refetchInterval: 3000,
  });
  if (status.isError)
    return (
      <Alert retry={() => void status.refetch()}>
        {t("deploymentUnavailable")}
      </Alert>
    );
  if (!status.data)
    return (
      <div className="grid gap-2" aria-busy="true">
        <span className="sr-only">{t("loading")}</span>
        <Skeleton className="h-8 w-full" />
        <Skeleton className="h-8 w-full" />
        <Skeleton className="h-8 w-2/3" />
      </div>
    );
  const data = status.data;
  if (!data.enabled) return <Notice>{t("deploymentDisabled")}</Notice>;
  return (
    <div className="grid min-w-0 gap-6">
      <Notice>
        {t("deploymentScope")}:{" "}
        <span className="font-mono text-foreground">
          {data.namespace} / {data.release}
        </span>
        . {t("deploymentReadOnly")}
      </Notice>
      <Panel title={t("deploymentReplicas")}>
        {!data.deployments.length ? (
          <Empty />
        ) : (
          <div className={tableFrame}>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("name")}</TableHead>
                  <TableHead className="text-right">
                    {t("deploymentDesired")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("deploymentReady")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("deploymentUpdated")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("deploymentAvailable")}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.deployments.map((d) => (
                  <TableRow key={d.name}>
                    <TableCell className="h-auto py-2 align-top">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-xs font-medium">
                          {d.name}
                        </span>
                        <StatusBadge tone="neutral">
                          {t(
                            d.role === "worker"
                              ? "deploymentWorker"
                              : d.role === "frontend"
                                ? "deploymentFrontend"
                                : "deploymentServer",
                          )}
                        </StatusBadge>
                        {!d.observed && (
                          <StatusBadge tone="warning">
                            {t("deploymentReconciling")}
                          </StatusBadge>
                        )}
                      </div>
                      <Conditions items={d.conditions} />
                    </TableCell>
                    <TableCell className={numberCell}>{d.desired}</TableCell>
                    <TableCell
                      className={`${numberCell} ${d.ready < d.desired ? "text-warning" : ""}`}
                    >
                      {d.ready}
                    </TableCell>
                    <TableCell className={numberCell}>{d.updated}</TableCell>
                    <TableCell className={numberCell}>{d.available}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </Panel>
      <Panel title={t("deploymentPods")}>
        {!data.pods.length ? (
          <Empty />
        ) : (
          <div className={tableFrame}>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("name")}</TableHead>
                  <TableHead>{t("deploymentReady")}</TableHead>
                  <TableHead className="text-right">
                    {t("deploymentRestarts")}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.pods.map((p) => (
                  <TableRow key={p.name}>
                    <TableCell className="h-auto py-2 align-top">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-xs font-medium">
                          {p.name}
                        </span>
                        <StatusBadge tone={phaseTones[p.phase] ?? "neutral"}>
                          {t("deploymentPhase" + p.phase)}
                        </StatusBadge>
                        {p.terminating && (
                          <StatusBadge tone="warning">
                            {t("deploymentDraining")}
                          </StatusBadge>
                        )}
                      </div>
                      <Conditions
                        items={p.conditions.filter((c) => c.status !== "True")}
                      />
                    </TableCell>
                    <TableCell className="h-auto py-2 align-top leading-5">
                      {t(
                        p.ready && !p.terminating
                          ? "deploymentYes"
                          : "deploymentNo",
                      )}
                    </TableCell>
                    <TableCell
                      className={`${numberCell} ${p.restarts > 0 ? "text-warning" : ""}`}
                    >
                      {p.restarts}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </Panel>
      <Panel title={t("deploymentEvents")}>
        {!data.events.length ? (
          <Empty />
        ) : (
          <ul className="divide-y divide-border rounded-md border border-border bg-surface">
            {data.events.map((e, i) => (
              <li key={i} className="grid min-w-0 gap-0.5 px-3 py-2">
                <div className="flex min-w-0 flex-wrap items-center gap-2 text-[13px]">
                  <StatusBadge
                    tone={e.kind === "Warning" ? "warning" : "neutral"}
                  >
                    {e.kind}
                  </StatusBadge>
                  <span className="font-mono text-xs font-medium">
                    {e.object}
                  </span>
                  <span className="text-muted-foreground">{e.reason}</span>
                  <span className="ml-auto font-mono text-[11px] text-faint tabular">
                    {e.time ? new Date(e.time).toLocaleString() : ""} · ×
                    {e.count}
                  </span>
                </div>
                <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                  {e.message}
                </p>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}
