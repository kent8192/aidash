import { useQuery } from "@tanstack/react-query";
import { deploymentStatus } from "./generated/aidash";
import { Badge, Empty, Panel, useI18n } from "./ui";

export function DeploymentPage() {
  const { t } = useI18n();
  const status = useQuery({
    queryKey: ["deployment"],
    queryFn: () => deploymentStatus(),
    refetchInterval: 3000,
  });
  if (status.isError)
    return (
      <div className="error" role="alert">
        <p>{t("deploymentUnavailable")}</p>
        <button onClick={() => void status.refetch()}>{t("retry")}</button>
      </div>
    );
  if (!status.data) return <p>{t("loading")}</p>;
  const data = status.data;
  if (!data.enabled) return <p className="notice">{t("deploymentDisabled")}</p>;
  return (
    <div className="deployment-page">
      <p className="notice">
        {t("deploymentScope")}: {data.namespace} / {data.release}.{" "}
        {t("deploymentReadOnly")}
      </p>
      <Panel title={t("deploymentReplicas")}>
        {!data.deployments.length && <Empty />}
        {data.deployments.map((d) => (
          <article className="deployment-card" key={d.name}>
            <h3>{d.name}</h3>
            <p>
              {t(
                d.role === "worker"
                  ? "deploymentWorker"
                  : d.role === "frontend"
                    ? "deploymentFrontend"
                    : "deploymentServer",
              )}
            </p>
            <dl>
              <dt>{t("deploymentDesired")}</dt>
              <dd>{d.desired}</dd>
              <dt>{t("deploymentReady")}</dt>
              <dd>{d.ready}</dd>
              <dt>{t("deploymentUpdated")}</dt>
              <dd>{d.updated}</dd>
              <dt>{t("deploymentAvailable")}</dt>
              <dd>{d.available}</dd>
            </dl>
            {!d.observed && (
              <p className="notice">{t("deploymentReconciling")}</p>
            )}
            {d.conditions.map((c, i) => (
              <p key={i}>
                <strong>
                  {c.kind}: {c.status} · {c.reason}
                </strong>{" "}
                {c.message}
              </p>
            ))}
          </article>
        ))}
      </Panel>
      <Panel title={t("deploymentPods")}>
        {!data.pods.length && <Empty />}
        {data.pods.map((p) => (
          <article className="deployment-card" key={p.name}>
            <h3>{p.name}</h3>
            <Badge value={"deploymentPhase" + p.phase} />
            <p>
              {t("deploymentReady")}:{" "}
              {t(p.ready && !p.terminating ? "deploymentYes" : "deploymentNo")}{" "}
              · {t("deploymentRestarts")}: {p.restarts}
            </p>
            {p.terminating && <p>{t("deploymentDraining")}</p>}
            {p.conditions
              .filter((c) => c.status !== "True")
              .map((c, i) => (
                <p key={i}>
                  <strong>
                    {c.kind}: {c.reason}
                  </strong>{" "}
                  {c.message}
                </p>
              ))}
          </article>
        ))}
      </Panel>
      <Panel title={t("deploymentEvents")}>
        {!data.events.length && <Empty />}
        {data.events.map((e, i) => (
          <article className="deployment-card" key={i}>
            <strong>
              {e.object} · {e.kind} · {e.reason}
            </strong>
            <p>{e.message}</p>
            <small>
              {e.time ? new Date(e.time).toLocaleString() : ""} · {e.count}
            </small>
          </article>
        ))}
      </Panel>
    </div>
  );
}
