import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  authorizationApproveCatalog,
  authorizationSetCatalog,
  registryVersions,
} from "./generated/aidash";
import type { RegistryVersion } from "./generated/models";
import type { State } from "./types";
import { ApiError } from "./transport";
import { keylessOrigin } from "./keyless";
import { deciderRecipients, type DeciderRecipient } from "./decider";
import { agentPlugins, type AgentPluginRef } from "./agent-plugins";
import { AgentPluginSummary } from "./agent-plugin-summary";
import { Badge, useEntryLabel, useI18n, type StatusTone } from "./ui";
import { Button } from "./components/ui/button";
import {
  Alert,
  Facts,
  GroupChoice,
  Hint,
  Notice,
  RowList,
} from "./components/patterns";
import {
  approvalRequest,
  approvalState,
  defaultAgentGroups,
  earlierAgentGroups,
  requestedGroups,
  unapprovedClosure,
  unapprovedDependency,
  type ApprovalState,
  type Closure,
} from "./catalog-approval-model";
import { useNodes } from "./nodes";
import { useTenantActions } from "./tenant-administration";

/** Every version in the signed-in subject's Tenant that it may read, approved
 *  or not. A Deployment Administrator reads no Tenant's Registry (ADR 0029). */
export function useRegistryVersions(access: State["access"]) {
  const subject = access.kind === "subject" ? access : null;
  return useQuery({
    queryKey: ["registry-versions", subject?.tenant, subject?.subject],
    queryFn: async () => {
      const all: RegistryVersion[] = [];
      let offset: number | null = 0;
      while (offset != null) {
        const page = await registryVersions({ offset });
        if (!Array.isArray(page.versions))
          throw new Error("Unexpected Registry page");
        all.push(...page.versions);
        offset = page.next_offset;
      }
      return all;
    },
    enabled: subject !== null,
  });
}

const approvalBadges: Record<ApprovalState, [string, StatusTone]> = {
  builtin: ["catalogBuiltin", "neutral"],
  approved: ["catalogApproved", "success"],
  withdrawn: ["catalogWithdrawn", "neutral"],
  unapproved: ["catalogUnapproved", "warning"],
};

export function ApprovalBadge({ version }: { version: RegistryVersion }) {
  const [label, tone] = approvalBadges[approvalState(version)];
  return <Badge value={label} tone={tone} />;
}

/** Where a Keyless definition sends requests (ADR 0036). */
export function KeylessOrigin({ origin }: { origin: string }) {
  const { t } = useI18n();
  return (
    <span className="keyless-origin text-[11px] text-warning [overflow-wrap:anywhere]">
      {t("authKeyless")} · <span className="font-mono">{origin}</span>
    </span>
  );
}

/** Every host a Decider version's Routes send Run state to (ADR 0066). */
export function DeciderRecipients({
  recipients,
}: {
  recipients: DeciderRecipient[];
}) {
  const { t } = useI18n();
  return (
    <span className="decider-recipients text-[11px] text-warning [overflow-wrap:anywhere]">
      {t("catalogDeciderRecipients")} ·{" "}
      {recipients.map((recipient, index) => (
        <span key={`${index}-${recipient.model}`}>
          {index > 0 && "; "}
          <span className="font-mono">{recipient.model}</span> (
          {recipient.hosts.join(", ")})
        </span>
      ))}
    </span>
  );
}

/** The Agent Plugins an Agent version takes in. Approving the version admits
 *  the servers they declare, so the approver reads each command or URL here
 *  and every part left out (ADR 0072, ADR 0073). */
export function AgentPluginsDisclosure({
  plugins,
  admits,
}: {
  plugins: AgentPluginRef[];
  /** The viewer is about to approve this version. */
  admits: boolean;
}) {
  const { t } = useI18n();
  const hasServers = plugins.some(
    (plugin) => Object.keys(plugin.servers).length > 0,
  );
  return (
    <section
      className="agent-plugins grid min-w-0 gap-2"
      aria-label={t("catalogAgentPlugins")}
    >
      <h5 className="text-xs font-semibold text-foreground">
        {t("catalogAgentPlugins")} ({plugins.length})
      </h5>
      {hasServers && (
        <Notice
          tone={admits ? "warning" : "info"}
          role="status"
          className="agent-plugins-notice"
        >
          {t("catalogAgentPluginsAdmit")} {t("agentPluginRuntime")}
        </Notice>
      )}
      <ul className="grid min-w-0 gap-3">
        {plugins.map((plugin) => (
          <li
            key={plugin.digest}
            className="min-w-0 rounded-md border border-border bg-surface px-3 py-2.5"
          >
            <AgentPluginSummary plugin={plugin} />
          </li>
        ))}
      </ul>
    </section>
  );
}

/** One version's Catalog Approval: its state and history, and for a subject
 *  holding `catalog.approve`, the decision itself. An approval refused for an
 *  unapproved dependency turns into one request for the whole closure. */
export function CatalogApprovalSection({
  version,
  versions,
  access,
}: {
  version: RegistryVersion;
  versions: RegistryVersion[];
  access: State["access"];
}) {
  const { t, locale } = useI18n();
  const client = useQueryClient();
  const label = useEntryLabel(versions.map((item) => item.entry));
  const { tenant, can, assignable } = useTenantActions(access);
  // The versions list reports the default Node's approvals, so that is the
  // Node a decision here applies to.
  const { home } = useNodes();
  const [chosen, setChosen] = useState<string[]>();
  const [closure, setClosure] = useState<Closure>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const entry = version.entry;
  const state = approvalState(version);
  const origin = keylessOrigin(entry);
  const recipients = deciderRecipients(entry);
  const plugins = agentPlugins(entry);
  const installed = !!entry.installation;
  const decides =
    tenant !== null &&
    home !== undefined &&
    can("catalog.approve") &&
    version.needs_approval &&
    !installed;
  const newSubject = entry.kind === "agent" && !version.subject;
  const choosesGroups =
    newSubject && can("subject.group.update") && assignable.length > 0;
  const groups = (
    chosen ??
    earlierAgentGroups(version, versions) ??
    defaultAgentGroups(assignable)
  ).filter((group) => assignable.includes(group));
  const sentGroups = choosesGroups
    ? requestedGroups(groups, assignable)
    : undefined;
  const path = encodeURIComponent(tenant ?? "");
  const decide = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      setClosure(undefined);
      await client.invalidateQueries();
    } catch (reason) {
      const named =
        reason instanceof ApiError && reason.status === 400
          ? unapprovedDependency(reason.message)
          : null;
      if (named) setClosure(unapprovedClosure(version, versions, named));
      else if (reason instanceof ApiError && reason.status === 409) {
        setError(t("catalogConflict"));
        await client.invalidateQueries({ queryKey: ["registry-versions"] });
      } else
        setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const approve = () =>
    decide(() =>
      authorizationSetCatalog(path, {
        ...approvalRequest(version, true, sentGroups),
        node: home!.name,
      }),
    );
  const withdraw = () =>
    decide(() =>
      authorizationSetCatalog(path, {
        ...approvalRequest(version, false),
        node: home!.name,
      }),
    );
  const approveClosure = (needed: Closure) =>
    decide(() =>
      authorizationApproveCatalog(path, {
        approvals: [
          ...needed.dependencies.map((item) => approvalRequest(item, true)),
          approvalRequest(version, true, sentGroups),
        ],
        node: home!.name,
      }),
    );
  const subheading = "text-xs font-semibold text-foreground";
  return (
    <section
      className="catalog-approval grid min-w-0 gap-3 border-t border-border pt-4"
      aria-label={t("catalogApproval")}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="text-[13px] font-semibold text-foreground">
          {t("catalogApproval")}
        </h4>
        <ApprovalBadge version={version} />
      </div>
      <Hint>
        {t(
          !version.needs_approval
            ? "catalogBuiltinHelp"
            : installed
              ? "catalogInstalledHelp"
              : "catalogHelp",
        )}
      </Hint>
      {origin &&
        (decides && state !== "approved" ? (
          <Notice tone="warning" role="status" className="keyless-notice">
            {t("authKeylessNotice")} <span className="font-mono">{origin}</span>
          </Notice>
        ) : (
          <KeylessOrigin origin={origin} />
        ))}
      {recipients && <DeciderRecipients recipients={recipients} />}
      {plugins && (
        <AgentPluginsDisclosure
          plugins={plugins}
          admits={decides && state !== "approved"}
        />
      )}
      {version.subject && (
        <Facts
          items={[
            [t("catalogAgentSubject"), version.subject.id, true],
            [
              t("catalogAgentGroups"),
              version.subject.groups.join(", ") || t("catalogNoGroups"),
              true,
            ],
            [
              t("status"),
              t(version.subject.enabled ? "enabled" : "authDisabled"),
            ],
          ]}
        />
      )}
      {decides && state !== "approved" && entry.kind === "agent" && (
        <>
          {!newSubject ? (
            <Hint>{t("catalogSubjectKept")}</Hint>
          ) : choosesGroups ? (
            <div
              className="grid gap-1.5"
              role="group"
              aria-label={t("catalogAgentGroups")}
            >
              <span className={subheading}>{t("catalogAgentGroups")}</span>
              <GroupChoice
                groups={assignable}
                selected={groups}
                disabled={busy}
                change={setChosen}
              />
              <Hint>{t("catalogAgentGroupsHelp")}</Hint>
            </div>
          ) : (
            <Hint>{t("catalogAgentDefault")}</Hint>
          )}
        </>
      )}
      {closure && (
        <div
          className="catalog-dependencies grid gap-2 rounded-md border border-border bg-surface px-3 py-2.5"
          role="group"
          aria-label={t("catalogDependencies")}
        >
          <h5 className={subheading}>{t("catalogDependencies")}</h5>
          <Hint>{t("catalogDependenciesHelp")}</Hint>
          <ul className="grid gap-1.5 text-xs">
            {[...closure.dependencies, version].map((item) => {
              const keyless = keylessOrigin(item.entry);
              const routes = deciderRecipients(item.entry);
              const itemPlugins = agentPlugins(item.entry);
              return (
                <li
                  key={`${item.entry.id}@${item.entry.version}`}
                  className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1"
                >
                  <span className="min-w-0 font-medium text-foreground [overflow-wrap:anywhere]">
                    {label(item.entry)}
                  </span>
                  <Badge value={item.entry.kind} />
                  {keyless && <KeylessOrigin origin={keyless} />}
                  {routes && <DeciderRecipients recipients={routes} />}
                  {itemPlugins && (
                    <div className="w-full">
                      <AgentPluginsDisclosure plugins={itemPlugins} admits />
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
          {closure.blocked.length > 0 && (
            <div className="grid gap-1">
              <p className="text-xs font-medium text-warning">
                {t("catalogBlocked")}
              </p>
              <ul className="grid gap-1 text-xs text-muted-foreground">
                {closure.blocked.map((blocked) => (
                  <li
                    key={`${blocked.reference.id}@${blocked.reference.version}`}
                  >
                    {label(blocked.reference)} ·{" "}
                    {t(
                      blocked.installed
                        ? "catalogBlockedInstalled"
                        : "catalogBlockedMissing",
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              disabled={busy || closure.blocked.length > 0}
              onClick={() => void approveClosure(closure)}
            >
              {t("catalogApproveAll")} ({closure.dependencies.length + 1})
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => setClosure(undefined)}
            >
              {t("cancel")}
            </Button>
          </div>
        </div>
      )}
      {decides && !closure && (
        <div className="flex flex-wrap gap-2">
          {state === "approved" ? (
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void withdraw()}
            >
              {t("catalogWithdraw")}
            </Button>
          ) : (
            <Button size="sm" disabled={busy} onClick={() => void approve()}>
              {t("catalogApprove")}
            </Button>
          )}
        </div>
      )}
      {error && <Alert>{error}</Alert>}
      {version.needs_approval && (
        <div className="grid gap-1.5">
          <h5 className={subheading}>{t("catalogHistory")}</h5>
          {version.history.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              {t("catalogNoHistory")}
            </p>
          ) : (
            <RowList>
              {[...version.history].reverse().map((record) => (
                <div
                  key={record.revision}
                  className="catalog-history grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-0.5 py-1.5 text-xs sm:grid-cols-[auto_minmax(0,1fr)_auto]"
                >
                  <Badge
                    value={
                      record.enabled ? "catalogApproved" : "catalogWithdrawn"
                    }
                    tone={record.enabled ? "success" : "neutral"}
                  />
                  <span className="min-w-0 truncate text-foreground">
                    {t("revision")}{" "}
                    <span className="font-mono tabular">{record.revision}</span>
                    <span className="text-muted-foreground">
                      {" · "}
                      {record.actor}
                    </span>
                  </span>
                  <time className="col-start-2 font-mono text-[11px] text-faint sm:col-start-auto">
                    {new Date(record.created_at).toLocaleString(locale)}
                  </time>
                </div>
              ))}
            </RowList>
          )}
        </div>
      )}
    </section>
  );
}
