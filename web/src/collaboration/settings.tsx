import { ReferenceName } from "../record-view";
import { RecordView } from "../record-view";
import type { ReactNode } from "react";
import type { Package, State } from "../types";
import { Badge, Panel, useI18n } from "../ui";
import { GenerationPage } from "../generation";
import { SemanticPage } from "../semantic";
import { AuthorizationPage } from "../authorization";
import { DeploymentPage } from "../deployment";
import { TransactionsPage } from "../transactions";
import { collaborationCopy } from "./copy";
import { settingsSections, type SettingsSection } from "./model";
import type { Selection } from "./details";

const operatorSections = new Set<SettingsSection>([
  "authorization",
  "transactions",
  "deployment",
  "marketplace",
]);

function SettingsFrame({
  section,
  select,
  operator,
  children,
}: {
  section: SettingsSection;
  select: (section: SettingsSection) => void;
  operator: boolean;
  children: ReactNode;
}) {
  const { t, locale } = useI18n();
  const copy = collaborationCopy[locale];
  return (
    <section className="collab-settings">
      <header>
        <h1>{copy.settings}</h1>
        <p>{copy.settingsHelp}</p>
      </header>
      <label className="collab-settings-select">
        {copy.settings}
        <select
          value={section}
          onChange={(event) => select(event.target.value as SettingsSection)}
        >
          {settingsSections
            .filter((value) => operator || !operatorSections.has(value))
            .map((value) => (
              <option value={value} key={value}>
                {value === "node" ? copy.node : t(value)}
              </option>
            ))}
        </select>
      </label>
      {children}
    </section>
  );
}

export function TransactionSettings({
  nodeId,
  select,
}: {
  nodeId: string;
  select: (section: SettingsSection) => void;
}) {
  return (
    <SettingsFrame section="transactions" select={select} operator>
      <TransactionsPage nodeId={nodeId} />
    </SettingsFrame>
  );
}

export function Configuration({
  data,
  section,
  select,
  open,
  packages,
  disconnect,
}: {
  data: State;
  section: SettingsSection;
  select: (section: SettingsSection) => void;
  open: (selection: Selection) => void;
  packages: Package[];
  disconnect: () => void;
}) {
  const { t, local, locale, entityName } = useI18n();
  const copy = collaborationCopy[locale];
  const operator = data.access.kind === "operator";
  const restricted = !operator && operatorSections.has(section);
  return (
    <SettingsFrame section={section} select={select} operator={operator}>
      {restricted ? (
        <p className="notice" role="status">
          {t("administratorsOnly")}
        </p>
      ) : (
        <>
          {section === "generation" && <GenerationPage data={data} />}
          {section === "semantic" && <SemanticPage data={data} />}
          {section === "authorization" && operator && (
            <AuthorizationPage entries={data.registry} />
          )}
          {section === "transactions" && operator && (
            <TransactionsPage nodeId={data.node.id} />
          )}
          {section === "deployment" && operator && <DeploymentPage />}
          {["agents", "registry", "clusters"].includes(section) && (
            <>
              {operator && (
                <button
                  className="primary"
                  type="button"
                  onClick={() =>
                    open({
                      kind: section === "clusters" ? "cluster" : "entity",
                    })
                  }
                >
                  {t("register")}
                </button>
              )}
              <div className="cards">
                {data.registry
                  .filter(
                    (entry) =>
                      section === "registry" ||
                      entry.kind ===
                        (section === "agents" ? "agent" : "cluster"),
                  )
                  .map((entry) => (
                    <button
                      type="button"
                      className="entity-card"
                      key={`${entry.kind}:${entry.id}@${entry.version}`}
                      onClick={() =>
                        open({ kind: "entityDetail", entity: entry })
                      }
                    >
                      <Badge value={entry.kind} />
                      <h3>{entityName(entry)}</h3>
                      <p>{local(entry.description)}</p>
                      <small>v{entry.version}</small>
                    </button>
                  ))}
              </div>
            </>
          )}
          {section === "marketplace" && operator && (
            <>
              <button
                className="primary"
                type="button"
                onClick={() => open({ kind: "publish" })}
              >
                {t("publish")}
              </button>
              <div className="cards">
                {packages.map((value) => (
                  <button
                    type="button"
                    className="entity-card"
                    key={`${value.id}@${value.version}`}
                    onClick={() => open({ kind: "package", package: value })}
                  >
                    <Badge value={value.manifest.entity.kind} />
                    <h3>{local(value.manifest.entity.name)}</h3>
                    <p>{local(value.manifest.entity.description)}</p>
                    <span>
                      {data.installations.some(
                        (installed) =>
                          installed.id === value.id &&
                          installed.version === value.version,
                      )
                        ? t("installed")
                        : t("details")}
                    </span>
                  </button>
                ))}
              </div>
            </>
          )}
          {section === "node" && (
            <>
              <Panel title={copy.node}>
                <dl>
                  <dt>{copy.node}</dt>
                  <dd>
                    <ReferenceName id={data.node.id} />
                  </dd>
                  <dt>{t("endpoint")}</dt>
                  <dd>{data.node.endpoint}</dd>
                  <dt>{t("protocol")}</dt>
                  <dd>{data.node.protocol_version}</dd>
                </dl>
              </Panel>
              <Panel title={t("signedInAs")}>
                {data.access.kind === "subject" ? (
                  <dl>
                    <dt>{t("tenant")}</dt>
                    <dd>{data.access.tenant}</dd>
                    <dt>{t("subject")}</dt>
                    <dd>{data.access.subject}</dd>
                  </dl>
                ) : (
                  <p>{t("administrator")}</p>
                )}
              </Panel>
              {operator && (
                <Panel
                  title={t("connectedNodes")}
                  action={
                    <button
                      type="button"
                      onClick={() => open({ kind: "peer" })}
                    >
                      {t("addPeer")}
                    </button>
                  }
                >
                  {data.peers.map((peer) => (
                    <div className="peer-row" key={peer.node_id}>
                      <strong>
                        <ReferenceName id={peer.node_id} />
                      </strong>
                      <p>{peer.endpoint}</p>
                      <Badge value={peer.enabled ? "ACTIVE" : "PAUSED"} />
                    </div>
                  ))}
                </Panel>
              )}
              <details>
                <summary>{t("metadata")}</summary>
                <RecordView value={data.node} />
              </details>
              <button type="button" onClick={disconnect}>
                {copy.disconnect}
              </button>
            </>
          )}
        </>
      )}
    </SettingsFrame>
  );
}
