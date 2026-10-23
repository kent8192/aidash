import {
  Bot,
  Boxes,
  ChevronRight,
  Container,
  KeyRound,
  Library,
  Network,
  Plus,
  Search,
  Server,
  ShieldCheck,
  Store,
  Waypoints,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Badge as Tag } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { NativeSelect } from "../components/ui/native-select";
import { focusedVersion } from "../approval-decider-model";
import { ApprovalSettingsPage } from "../approval-settings";
import { approvalCopy } from "../approval-settings-copy";
import { DeploymentPage } from "../deployment";
import { UsagePanel } from "@slots/usage";
import {
  Alert,
  Disclosure,
  Facts,
  Loading,
  Notice,
  ScreenHeader,
} from "../components/patterns";
import { ToolSection } from "../integrated-tools";
import { keylessOrigin } from "../keyless";
import { groupByKind } from "../registry-kinds";
import {
  ApprovalBadge,
  KeylessOrigin,
  useRegistryVersions,
} from "../catalog-approval";
import type { RegistryVersion } from "../generated/models";
import { useNodes } from "../nodes";
import { NodesPage } from "../nodes-settings";
import { registryDeployment } from "../node-selection";
import { useTenantActions } from "../tenant-administration";
import { WorkbenchTestSettings } from "../workbench-test-settings";
import { cn } from "../lib/utils";
import { MarketplaceAdministration, ScopedMarketplace } from "../marketplace";
import { ModelSource } from "../model-source";
import { PlanConnectionsPage } from "../plan-connections";
import { ProviderCredentialsPage } from "../provider-credentials";
import { RecordView } from "../record-view";
import { SemanticPage } from "../semantic";
import type { State } from "../types";
import { Badge, Empty, Panel, useEntityName, useI18n } from "../ui";
import { collaborationCopy } from "./copy";
import type { Selection } from "./details";
import {
  personalSettingsSections,
  visibleSettingsSections,
  type IntegratedView,
  type SettingsSection,
} from "./model";

const deploymentAdministratorSections: Partial<Record<SettingsSection, true>> =
  {
    deployment: true,
  };

const sectionIcons: Partial<Record<SettingsSection, LucideIcon>> = {
  node: Server,
  nodes: Waypoints,
  agents: Bot,
  registry: Library,
  clusters: Network,
  deployment: Container,
  providerCredentials: KeyRound,
  planConnections: KeyRound,
  marketplace: Store,
  approvals: ShieldCheck,
};

const listCopy = {
  "ja-JP": {
    filter: "名前・タグで絞り込む",
    entries: (count: number) => `${count} 件`,
    noMatches: "条件に一致するエントリはありません。",
    kinds: "種類で表示を切り替える",
  },
  "en-US": {
    filter: "Filter by name or tag",
    entries: (count: number) => `${count} ${count === 1 ? "entry" : "entries"}`,
    noMatches: "No entries match this filter.",
    kinds: "Show entries of one kind",
  },
};

function useSectionLabel() {
  const { t, locale } = useI18n();
  return (section: SettingsSection) =>
    section === "node" ? (locale === "ja-JP" ? "一般" : "General") : t(section);
}

function SettingsFrame({
  section,
  select,
  deploymentAdministrator,
  children,
}: {
  section: SettingsSection;
  select: (section: SettingsSection) => void;
  deploymentAdministrator: boolean;
  children: ReactNode;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const label = useSectionLabel();
  const sections = visibleSettingsSections.filter((value) =>
    deploymentAdministrator
      ? !personalSettingsSections[value]
      : !deploymentAdministratorSections[value],
  );
  return (
    <section className="collab-settings flex h-full min-h-0 flex-col">
      <ScreenHeader
        title={copy.settings}
        section={label(section)}
        description={copy.settingsHelp}
      />
      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <nav
          aria-label={copy.settings}
          className="hidden w-52 shrink-0 overflow-y-auto border-r border-border bg-surface p-2 md:block"
        >
          <ul className="grid gap-px">
            {sections.map((value) => {
              const Icon = sectionIcons[value] ?? Boxes;
              const current = value === section;
              return (
                <li key={value}>
                  <button
                    type="button"
                    aria-current={current ? "page" : undefined}
                    onClick={() => select(value)}
                    className={cn(
                      "relative flex h-8 w-full items-center gap-2 rounded-md px-2.5 text-left text-[13px] transition-colors duration-150",
                      current
                        ? "bg-brand-soft font-medium text-foreground before:absolute before:inset-y-1.5 before:left-0 before:w-0.5 before:rounded-full before:bg-brand-mark"
                        : "text-muted-foreground hover:bg-accent hover:text-foreground",
                    )}
                  >
                    <Icon
                      aria-hidden
                      className={cn("size-4 shrink-0", current && "text-brand")}
                    />
                    <span className="truncate">{label(value)}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </nav>
        <div className="shrink-0 border-b border-border px-4 py-2 md:hidden">
          <label className="collab-settings-select flex items-center gap-3">
            <span className="shrink-0 text-xs font-medium text-muted-foreground">
              {copy.settings}
            </span>
            <NativeSelect
              value={section}
              onChange={(event) =>
                select(event.target.value as SettingsSection)
              }
            >
              {sections.map((value) => (
                <option value={value} key={value}>
                  {label(value)}
                </option>
              ))}
            </NativeSelect>
          </label>
        </div>
        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto">
          <div className="grid min-w-0 gap-6 px-4 py-5 md:px-6">{children}</div>
        </div>
      </div>
    </section>
  );
}

function initials(name: string) {
  const words = name.split(/[\s_-]+/).filter(Boolean);
  return (
    (words.length > 1 ? words[0][0] + words[1][0] : name.slice(0, 2)) || "?"
  ).toUpperCase();
}

const columns =
  "md:grid-cols-[1.75rem_minmax(0,1fr)_7rem_minmax(0,10rem)_7.5rem_5.5rem_1rem]";
const rowClass = cn(
  "entity-card group grid w-full min-w-0 cursor-pointer grid-cols-[1.75rem_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 px-3 py-2 text-left transition-colors duration-150 hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
  columns,
);
const headClass = cn(
  "hidden h-8 items-center gap-x-3 border-b border-border px-3 text-[11px] font-medium text-faint md:grid",
  columns,
);

function EntryRow({
  name,
  description,
  keyless,
  source,
  kind,
  tags,
  approval,
  trailing,
  onOpen,
}: {
  name: string;
  description: string;
  keyless: string | null;
  source: ReactNode;
  kind: string;
  tags: readonly string[];
  approval: ReactNode;
  trailing: ReactNode;
  onOpen: () => void;
}) {
  return (
    <li>
      <button type="button" className={rowClass} onClick={onOpen}>
        <span
          aria-hidden
          className="row-span-2 grid size-7 place-items-center rounded-md bg-raised font-mono text-[11px] font-medium text-muted-foreground md:row-span-1"
        >
          {initials(name)}
        </span>
        <span className="min-w-0">
          <h3 className="truncate text-[13px] font-medium text-foreground">
            {name}
          </h3>
          {description && (
            <p className="truncate text-xs text-muted-foreground">
              {description}
            </p>
          )}
          {source}
          {keyless && (
            <p className="truncate">
              <KeylessOrigin origin={keyless} />
            </p>
          )}
        </span>
        <span className="col-start-2 row-start-2 flex min-w-0 flex-wrap items-center gap-2 md:col-start-auto md:row-start-auto">
          <Badge value={kind} />
          <span className="md:hidden">{approval}</span>
          <span className="font-mono text-xs text-muted-foreground tabular md:hidden">
            {trailing}
          </span>
        </span>
        <span className="hidden min-w-0 items-center gap-1 overflow-hidden md:flex">
          {tags.slice(0, 3).map((tag) => (
            <Tag key={tag} tone="neutral" className="max-w-[8rem] truncate">
              {tag}
            </Tag>
          ))}
          {tags.length > 3 && (
            <span className="font-mono text-[11px] text-faint">
              +{tags.length - 3}
            </span>
          )}
        </span>
        <span className="hidden min-w-0 md:flex">{approval}</span>
        <span className="hidden truncate font-mono text-xs text-muted-foreground tabular md:block">
          {trailing}
        </span>
        <ChevronRight
          aria-hidden
          className="col-start-3 row-span-2 row-start-1 size-4 text-faint transition-colors group-hover:text-foreground md:col-start-auto md:row-span-1 md:row-start-auto"
        />
      </button>
    </li>
  );
}

function EntryList({
  versions,
  open,
  action,
}: {
  versions: RegistryVersion[];
  open: (selection: Selection) => void;
  action?: ReactNode;
}) {
  const { t, local, locale } = useI18n();
  const entries = versions.map((version) => version.entry);
  const entityName = useEntityName(entries);
  const copy = listCopy[locale];
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  const matching = versions.filter(
    ({ entry }) =>
      !needle ||
      [
        entityName(entry),
        entry.kind,
        ...(entry.tags ?? []),
        ...(entry.capabilities ?? []),
      ]
        .join(" ")
        .toLowerCase()
        .includes(needle),
  );
  // One kind at a time: a Registry mixing Models, Tools and Agents in one list
  // is hard to scan. "all" stays available for searching across kinds.
  const groups = groupByKind(matching, ({ entry }) => entry.kind);
  const [chosenKind, setKind] = useState<string | null>(null);
  const kind =
    chosenKind === "all" || groups.some(([group]) => group === chosenKind)
      ? chosenKind
      : (groups[0]?.[0] ?? "all");
  const shown =
    groups.length > 1 && kind !== "all"
      ? matching.filter(({ entry }) => entry.kind === kind)
      : matching;
  return (
    <section className="grid min-w-0 gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <label className="relative w-full sm:w-72">
          <span className="sr-only">{copy.filter}</span>
          <Search
            aria-hidden
            className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
          />
          <Input
            type="search"
            value={query}
            placeholder={copy.filter}
            onChange={(event) => setQuery(event.target.value)}
            className="pl-8"
          />
        </label>
        <span className="font-mono text-xs text-faint tabular">
          {copy.entries(shown.length)}
        </span>
        <span className="ml-auto">{action}</span>
      </div>
      {groups.length > 1 && (
        <div
          role="group"
          aria-label={copy.kinds}
          className="flex flex-wrap items-center gap-1.5"
        >
          {[...groups, ["all", matching] as const].map(([group, items]) => (
            <Button
              key={group}
              type="button"
              size="sm"
              variant={group === kind ? "secondary" : "ghost"}
              aria-pressed={group === kind}
              data-registry-kind={group}
              onClick={() => setKind(group)}
            >
              {t(group)}
              <span className="font-mono text-[11px] text-faint tabular">
                {items.length}
              </span>
            </Button>
          ))}
        </div>
      )}
      {entries.length === 0 ? (
        <Empty />
      ) : (
        <div className="min-w-0 rounded-md border border-border bg-surface">
          <div aria-hidden className={headClass}>
            <span />
            <span>{t("name")}</span>
            <span>{t("kind")}</span>
            <span>{t("tags")}</span>
            <span>{t("catalogApprovalColumn")}</span>
            <span>{t("version")}</span>
            <span />
          </div>
          {shown.length === 0 ? (
            <p className="px-3 py-6 text-xs text-muted-foreground">
              {copy.noMatches}
            </p>
          ) : (
            <ul className="divide-y divide-border">
              {shown.map((version) => (
                <EntryRow
                  key={`${version.entry.kind}:${version.entry.id}@${version.entry.version}`}
                  name={entityName(version.entry)}
                  description={local(version.entry.description)}
                  keyless={keylessOrigin(version.entry)}
                  source={<ModelSource entry={version.entry} />}
                  kind={version.entry.kind}
                  tags={version.entry.tags ?? []}
                  approval={<ApprovalBadge version={version} />}
                  trailing={version.entry.version}
                  onOpen={() =>
                    open({ kind: "entityDetail", entity: version.entry })
                  }
                />
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

function NodeView({
  data,
  disconnect,
}: {
  data: State;
  disconnect: () => void;
}) {
  const { t, locale } = useI18n();
  const copy = collaborationCopy[locale];
  const { selected } = useNodes();
  const deployment = registryDeployment(data);
  return (
    <div className="grid max-w-4xl min-w-0 gap-6">
      <Panel title={copy.node}>
        <Facts
          items={[
            [t("deploymentId"), deployment, true],
            ...(selected
              ? ([[t("node"), selected.label]] as [string, string][])
              : []),
            [t("endpoint"), data.node.endpoint, true],
            [t("protocol"), data.deployment.protocol_version, true],
          ]}
        />
      </Panel>
      <Panel
        title={t("signedInAs")}
        action={
          <Button
            variant="outline"
            size="sm"
            type="button"
            onClick={disconnect}
          >
            {copy.disconnect}
          </Button>
        }
      >
        {data.access.kind === "subject" ? (
          <Facts
            items={[
              [t("tenant"), data.access.tenant, true],
              [t("subject"), data.access.subject, true],
            ]}
          />
        ) : (
          <p className="text-[13px] text-foreground">{t("administrator")}</p>
        )}
      </Panel>
      <Disclosure summary={t("metadata")}>
        <RecordView value={data.node} />
      </Disclosure>
    </div>
  );
}

/** The subject's own Tenant Registry: every version with its Catalog
 *  Approval, and registration for subjects holding `registry.create`. */
function TenantRegistry({
  data,
  section,
  open,
  focus,
}: {
  data: State;
  section: "agents" | "registry" | "clusters";
  open: (selection: Selection) => void;
  /** `<id>@<version>` of a version a link asks to see, such as the Tenant's
   *  approval Decider whose Catalog Approval Auto needs. */
  focus?: string;
}) {
  const { t } = useI18n();
  const versions = useRegistryVersions(data.access);
  const { can } = useTenantActions(data.access);
  const opened = useRef("");
  useEffect(() => {
    if (!focus || !versions.data || opened.current === focus) return;
    opened.current = focus;
    const version = focusedVersion(versions.data, focus);
    if (version) open({ kind: "entityDetail", entity: version.entry });
  }, [focus, versions.data, open]);
  if (versions.isError) return <Alert>{t("catalogRegistryUnavailable")}</Alert>;
  if (!versions.data) return <Loading />;
  const kind = { agents: "agent", clusters: "cluster", registry: null }[
    section
  ];
  return (
    <EntryList
      versions={versions.data.filter(
        (version) => kind === null || version.entry.kind === kind,
      )}
      open={open}
      action={
        can("registry.create") && (
          <Button
            type="button"
            onClick={() =>
              open({ kind: section === "clusters" ? "cluster" : "entity" })
            }
          >
            <Plus aria-hidden />
            {t("register")}
          </Button>
        )
      }
    />
  );
}

export function Configuration({
  data,
  section,
  select,
  open,
  disconnect,
  integration,
  channel,
  focus,
}: {
  data: State;
  section: SettingsSection;
  select: (section: SettingsSection) => void;
  open: (selection: Selection) => void;
  disconnect: () => void;
  integration?: IntegratedView;
  channel?: string;
  focus?: string;
}) {
  const { t, locale } = useI18n();
  const { can } = useTenantActions(data.access);
  const deploymentAdministrator =
    data.access.kind === "deployment_administrator";
  const restricted =
    !deploymentAdministrator &&
    deploymentAdministratorSections[section] === true;
  return (
    <SettingsFrame
      section={section}
      select={select}
      deploymentAdministrator={deploymentAdministrator}
    >
      {restricted ? (
        <Notice role="status">{t("administratorsOnly")}</Notice>
      ) : (
        <>
          {section === "deployment" && deploymentAdministrator && (
            <>
              <DeploymentPage />
              <UsagePanel />
            </>
          )}
          {section === "planConnections" &&
            (data.access.kind === "subject" ? (
              <PlanConnectionsPage
                key={`${data.node.id}:${data.access.tenant}:${data.access.subject}`}
                tenant={data.access.tenant}
                subject={data.access.subject}
              />
            ) : (
              <Notice role="status">
                {locale === "ja-JP"
                  ? "Plan 接続はログイン中の Subject 個人の設定です。"
                  : "Plan Connections belong to a signed-in subject."}
              </Notice>
            ))}
          {section === "providerCredentials" && (
            <ProviderCredentialsPage access={data.access} />
          )}
          {(section === "agents" ||
            section === "registry" ||
            section === "clusters") &&
            (deploymentAdministrator ? (
              <Notice role="status">{t("catalogTenantDefinitions")}</Notice>
            ) : (
              <TenantRegistry
                key={section}
                data={data}
                section={section}
                open={open}
                focus={section === "registry" ? focus : undefined}
              />
            ))}
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
          {section === "registry" &&
            data.access.kind === "subject" &&
            (can("workbench.test_limits.update") ||
              can("workbench.test_profile.update")) && (
              <ToolSection title={t("workbenchTests")}>
                <WorkbenchTestSettings tenant={data.access.tenant} can={can} />
              </ToolSection>
            )}
          {section === "marketplace" &&
            !deploymentAdministrator &&
            data.access.kind === "subject" && (
              <ScopedMarketplace
                key={`${data.node.id}:${data.access.tenant}:${data.access.subject}`}
                identity={`${data.node.id}:${data.access.tenant}:${data.access.subject}`}
                tenant={data.access.tenant}
                approves={can("catalog.approve")}
              />
            )}
          {section === "marketplace" && deploymentAdministrator && (
            <MarketplaceAdministration />
          )}
          {section === "approvals" &&
            (data.access.kind === "subject" ? (
              <ApprovalSettingsPage
                key={`${data.node.id}:${data.access.tenant}:${data.access.subject}`}
                tenant={data.access.tenant}
              />
            ) : (
              <Notice role="status">{approvalCopy[locale].personalOnly}</Notice>
            ))}
          {section === "node" && (
            <NodeView data={data} disconnect={disconnect} />
          )}
          {section === "nodes" && <NodesPage data={data} />}
        </>
      )}
    </SettingsFrame>
  );
}
