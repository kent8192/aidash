import { Button } from "../components/ui/button";
import { ScopedMarketplace, MarketplaceAdministration } from "../marketplace";
import { ToolSection } from "../integrated-tools";
import { ReferenceName } from "../record-view";
import { RecordView } from "../record-view";
import type { ReactNode } from "react";
import type { Package, State } from "../types";
import { Badge, Panel, useEntityName, useI18n } from "../ui";

import { SemanticPage } from "../semantic";

import { ProviderCredentialsPage } from "../provider-credentials";

import { DeploymentPage } from "../deployment";

import { collaborationCopy } from "./copy";
import {
  visibleSettingsSections,
  type SettingsSection,
  type IntegratedView,
} from "./model";
import type { Selection } from "./details";

const operatorSections = new Set<SettingsSection>(["deployment"]);

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
          {visibleSettingsSections
            .filter((value) => operator || !operatorSections.has(value))
            .map((value) => (
              <option value={value} key={value}>
                {value === "node"
                  ? locale === "ja-JP"
                    ? "一般"
                    : "General"
                  : t(value)}
              </option>
            ))}
        </select>
      </label>
      {children}
    </section>
  );
}

export function Configuration({
  data,
  section,
  select,
  open,
  packages,
  disconnect,
  integration,
  channel,
}: {
  data: State;
  section: SettingsSection;
  select: (section: SettingsSection) => void;
  open: (selection: Selection) => void;
  packages: Package[];
  disconnect: () => void;
  integration?: IntegratedView;
  channel?: string;
}) {
  const { t, local, locale } = useI18n();
  const entityName = useEntityName(data.registry);
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
          {section === "deployment" && operator && <DeploymentPage />}
          {section === "providerCredentials" && (
            <ProviderCredentialsPage access={data.access} />
          )}
          {["agents", "registry", "clusters"].includes(section) && (
            <>
              {operator && (
                <Button
                  variant="outline"
                  className="primary"
                  type="button"
                  onClick={() =>
                    open({
                      kind: section === "clusters" ? "cluster" : "entity",
                    })
                  }
                >
                  {t("register")}
                </Button>
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
                    <Button
                      variant="outline"
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
                    </Button>
                  ))}
              </div>
            </>
          )}
          {section === "registry" && (
            <ToolSection
              title={
                locale === "ja-JP" ? "検索ソースと索引" : "Sources and indexes"
              }
              initiallyOpen={integration === "semantic"}
            >
              <SemanticPage data={data} workspaceId={channel} />
            </ToolSection>
          )}
          {section === "marketplace" &&
            !operator &&
            data.access.kind === "subject" && (
              <ScopedMarketplace
                key={`${data.node.id}:${data.access.tenant}:${data.access.subject}`}
                identity={`${data.node.id}:${data.access.tenant}:${data.access.subject}`}
              />
            )}
          {section === "marketplace" && operator && (
            <MarketplaceAdministration />
          )}
          {section === "marketplace" && operator && (
            <>
              <Button
                variant="outline"
                className="primary"
                type="button"
                onClick={() => open({ kind: "publish" })}
              >
                {t("publish")}
              </Button>
              <div className="cards">
                {packages.map((value) => (
                  <Button
                    variant="outline"
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
                  </Button>
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
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => open({ kind: "peer" })}
                    >
                      {t("addPeer")}
                    </Button>
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
              <Button variant="outline" type="button" onClick={disconnect}>
                {copy.disconnect}
              </Button>
            </>
          )}
        </>
      )}
    </SettingsFrame>
  );
}
