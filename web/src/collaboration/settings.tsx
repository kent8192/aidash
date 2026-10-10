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
  Store,
  type LucideIcon,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import { Badge as Tag } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { NativeSelect } from "../components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../components/ui/table";
import { DeploymentPage } from "../deployment";
import { Disclosure, Facts, Notice, ScreenHeader } from "../components/patterns";
import { ToolSection } from "../integrated-tools";
import { cn } from "../lib/utils";
import { MarketplaceAdministration, ScopedMarketplace } from "../marketplace";
import { ProviderCredentialsPage } from "../provider-credentials";
import { RecordView, ReferenceName } from "../record-view";
import { SemanticPage } from "../semantic";
import type { Entry, Package, State } from "../types";
import { Badge, Empty, Panel, useEntityName, useI18n } from "../ui";
import { collaborationCopy } from "./copy";
import type { Selection } from "./details";
import {
  visibleSettingsSections,
  type IntegratedView,
  type SettingsSection,
} from "./model";

const operatorSections: Partial<Record<SettingsSection, true>> = {
  deployment: true,
};

const sectionIcons: Partial<Record<SettingsSection, LucideIcon>> = {
  node: Server,
  agents: Bot,
  registry: Library,
  clusters: Network,
  deployment: Container,
  providerCredentials: KeyRound,
  marketplace: Store,
};

const listCopy = {
  "ja-JP": {
    filter: "名前・タグで絞り込む",
    entries: (count: number) => `${count} 件`,
    noMatches: "条件に一致するエントリはありません。",
    packages: "公開済みパッケージ",
  },
  "en-US": {
    filter: "Filter by name or tag",
    entries: (count: number) => `${count} ${count === 1 ? "entry" : "entries"}`,
    noMatches: "No entries match this filter.",
    packages: "Published packages",
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
  operator,
  children,
}: {
  section: SettingsSection;
  select: (section: SettingsSection) => void;
  operator: boolean;
  children: ReactNode;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const label = useSectionLabel();
  const sections = visibleSettingsSections.filter(
    (value) => operator || !operatorSections[value],
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

const rowClass =
  "entity-card group grid w-full min-w-0 cursor-pointer grid-cols-[1.75rem_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 px-3 py-2 text-left transition-colors duration-150 hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring md:grid-cols-[1.75rem_minmax(0,1fr)_7rem_minmax(0,12rem)_5.5rem_1rem]";
const headClass =
  "hidden h-8 items-center gap-x-3 border-b border-border px-3 text-[11px] font-medium text-faint md:grid md:grid-cols-[1.75rem_minmax(0,1fr)_7rem_minmax(0,12rem)_5.5rem_1rem]";

function EntryRow({
  name,
  description,
  kind,
  tags,
  trailing,
  onOpen,
}: {
  name: string;
  description: string;
  kind: string;
  tags: readonly string[];
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
        </span>
        <span className="col-start-2 row-start-2 flex min-w-0 items-center gap-2 md:col-start-auto md:row-start-auto">
          <Badge value={kind} />
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
  entries,
  open,
  action,
}: {
  entries: Entry[];
  open: (selection: Selection) => void;
  action?: ReactNode;
}) {
  const { t, local, locale } = useI18n();
  const entityName = useEntityName(entries);
  const copy = listCopy[locale];
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  const shown = entries.filter(
    (entry) =>
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
      {entries.length === 0 ? (
        <Empty />
      ) : (
        <div className="min-w-0 rounded-md border border-border bg-surface">
          <div aria-hidden className={headClass}>
            <span />
            <span>{t("name")}</span>
            <span>{t("kind")}</span>
            <span>{t("tags")}</span>
            <span>{t("version")}</span>
            <span />
          </div>
          {shown.length === 0 ? (
            <p className="px-3 py-6 text-xs text-muted-foreground">
              {copy.noMatches}
            </p>
          ) : (
            <ul className="divide-y divide-border">
              {shown.map((entry) => (
                <EntryRow
                  key={`${entry.kind}:${entry.id}@${entry.version}`}
                  name={entityName(entry)}
                  description={local(entry.description)}
                  kind={entry.kind}
                  tags={entry.tags ?? []}
                  trailing={entry.version}
                  onOpen={() => open({ kind: "entityDetail", entity: entry })}
                />
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

function PackageList({
  packages,
  data,
  open,
  action,
}: {
  packages: Package[];
  data: State;
  open: (selection: Selection) => void;
  action: ReactNode;
}) {
  const { t, local, locale } = useI18n();
  return (
    <section className="grid min-w-0 gap-3">
      <div className="flex items-center gap-2">
        <h2 className="text-[13px] font-semibold text-foreground">
          {listCopy[locale].packages}
        </h2>
        <span className="font-mono text-xs text-faint tabular">
          {packages.length}
        </span>
        <span className="ml-auto">{action}</span>
      </div>
      {packages.length === 0 ? (
        <Empty />
      ) : (
        <ul className="divide-y divide-border rounded-md border border-border bg-surface">
          {packages.map((value) => {
            const installed = data.installations.some(
              (item) => item.id === value.id && item.version === value.version,
            );
            return (
              <EntryRow
                key={`${value.id}@${value.version}`}
                name={local(value.manifest.entity.name)}
                description={local(value.manifest.entity.description)}
                kind={value.manifest.entity.kind}
                tags={[`v${value.version}`]}
                trailing={installed ? t("installed") : t("details")}
                onOpen={() => open({ kind: "package", package: value })}
              />
            );
          })}
        </ul>
      )}
    </section>
  );
}

function NodeView({
  data,
  open,
  disconnect,
}: {
  data: State;
  open: (selection: Selection) => void;
  disconnect: () => void;
}) {
  const { t, locale } = useI18n();
  const copy = collaborationCopy[locale];
  const operator = data.access.kind === "operator";
  return (
    <div className="grid max-w-4xl min-w-0 gap-6">
      <Panel title={copy.node}>
        <Facts
          items={[
            [copy.node, <ReferenceName id={data.node.id} />],
            [t("endpoint"), data.node.endpoint, true],
            [t("protocol"), data.node.protocol_version, true],
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
      {operator && (
        <Panel
          title={t("connectedNodes")}
          action={
            <Button
              variant="outline"
              size="sm"
              type="button"
              onClick={() => open({ kind: "peer" })}
            >
              <Plus aria-hidden />
              {t("addPeer")}
            </Button>
          }
        >
          {data.peers.length === 0 ? (
            <Empty />
          ) : (
            <div className="min-w-0 rounded-md border border-border bg-surface">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t("node")}</TableHead>
                    <TableHead>{t("endpoint")}</TableHead>
                    <TableHead>{t("protocol")}</TableHead>
                    <TableHead className="text-right" />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {data.peers.map((peer) => (
                    <TableRow key={peer.node_id} className="peer-row">
                      <TableCell className="font-medium">
                        <ReferenceName id={peer.node_id} />
                      </TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        {peer.endpoint}
                      </TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        {peer.protocol_version}
                      </TableCell>
                      <TableCell className="text-right">
                        <Badge value={peer.enabled ? "ACTIVE" : "PAUSED"} />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </Panel>
      )}
      <Disclosure summary={t("metadata")}>
        <RecordView value={data.node} />
      </Disclosure>
    </div>
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
  const { t, locale } = useI18n();
  const operator = data.access.kind === "operator";
  const restricted = !operator && operatorSections[section] === true;
  const registerAction = operator && (
    <Button
      type="button"
      onClick={() =>
        open({ kind: section === "clusters" ? "cluster" : "entity" })
      }
    >
      <Plus aria-hidden />
      {t("register")}
    </Button>
  );
  return (
    <SettingsFrame section={section} select={select} operator={operator}>
      {restricted ? (
        <Notice role="status">{t("administratorsOnly")}</Notice>
      ) : (
        <>
          {section === "deployment" && operator && <DeploymentPage />}
          {section === "providerCredentials" && (
            <ProviderCredentialsPage access={data.access} />
          )}
          {(section === "agents" ||
            section === "registry" ||
            section === "clusters") && (
            <EntryList
              key={section}
              entries={data.registry.filter(
                (entry) =>
                  section === "registry" ||
                  entry.kind === (section === "agents" ? "agent" : "cluster"),
              )}
              open={open}
              action={registerAction}
            />
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
            <>
              <MarketplaceAdministration />
              <PackageList
                packages={packages}
                data={data}
                open={open}
                action={
                  <Button
                    type="button"
                    onClick={() => open({ kind: "publish" })}
                  >
                    <Plus aria-hidden />
                    {t("publish")}
                  </Button>
                }
              />
            </>
          )}
          {section === "node" && (
            <NodeView data={data} open={open} disconnect={disconnect} />
          )}
        </>
      )}
    </SettingsFrame>
  );
}
