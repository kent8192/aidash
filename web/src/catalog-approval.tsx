import { RegisteredCompactionDetails } from "./compaction-details";
import { SecretDisclosure } from "./tenant-secrets";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { authorizationSetCatalog, registryVersions } from "./generated/aidash";
import type { RegistryVersion } from "./generated/models";
import type { EntityRef, Entry, State } from "./types";
import { ApiError } from "./transport";
import { keylessOrigin } from "./keyless";
import { toolReplay } from "./tool-declaration";
import { deciderRecipients, type DeciderRecipient } from "./decider";
import { agentPlugins, type AgentPluginRef } from "./agent-plugins";
import { AgentPluginSummary } from "./agent-plugin-summary";
import { Badge, useI18n, type StatusTone } from "./ui";
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
  restrictionLabel,
  type ApprovalState,
  type Restriction,
} from "./catalog-approval-model";
import { ApprovalReviewPanel } from "./approval-review";
import { useNodes } from "./nodes";
import { useTenantActions } from "./tenant-administration";

/** Every version in the signed-in subject's Tenant that it may read, with the
 *  approvals of the Node it works in. A Deployment Administrator reads no
 *  Tenant's Registry (ADR 0029). */
export function useRegistryVersions(access: State["access"]) {
  const subject = access.kind === "subject" ? access : null;
  const node = useNodes().selected?.name;
  return useQuery({
    queryKey: ["registry-versions", subject?.tenant, subject?.subject, node],
    queryFn: async () => {
      const all: RegistryVersion[] = [];
      let offset: number | null = 0;
      while (offset != null) {
        const page = await registryVersions({ node: node!, offset });
        if (!Array.isArray(page.versions))
          throw new Error("Unexpected Registry page");
        all.push(...page.versions);
        offset = page.next_offset;
      }
      return all;
    },
    enabled: subject !== null && node !== undefined,
  });
}

const approvalBadges: Record<ApprovalState, [string, StatusTone]> = {
  builtin: ["catalogBuiltin", "neutral"],
  approved: ["catalogApproved", "success"],
  withdrawn: ["catalogWithdrawn", "neutral"],
  unapproved: ["catalogUnapproved", "warning"],
};

/** A Run start refused because the agent's compaction Decider is not
 *  approved on this Node (ADR 0102). It names the Decider and opens it, where
 *  it can be approved or its approval requested (ADR 0096). */
export function DeciderStartRefusal({
  decider,
  data,
  openEntry,
}: {
  decider: EntityRef;
  data: State;
  openEntry: (entry: Entry) => void;
}) {
  const { t, entityLabel } = useI18n();
  const versions = useRegistryVersions(data.access);
  const entry = versions.data?.find(
    (value) =>
      value.entry.id === decider.id && value.entry.version === decider.version,
  )?.entry;
  return (
    <Alert>
      <p>{t("catalogDeciderUnapproved")}</p>
      <p>{entry ? entityLabel(entry) : `${decider.id}@${decider.version}`}</p>
      <p>{t("catalogDeciderUnapprovedHelp")}</p>
      {entry && (
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="justify-self-start"
          onClick={() => openEntry(entry)}
        >
          {t("catalogOpenDecider")}
        </Button>
      )}
    </Alert>
  );
}

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

/** One version's Catalog Approval: its state and history, a single-item
 *  decision for a subject holding `catalog.approve`, and the server-driven
 *  review for the whole closure. */
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
  const { tenant, can, assignable } = useTenantActions(access);
  // The versions list reports the selected Node's approvals, so that is the
  // Node a decision here applies to.
  const { selected } = useNodes();
  const [chosen, setChosen] = useState<string[]>();
  const [restricting, setRestricting] = useState<boolean>();
  const [subjectNames, setSubjectNames] = useState<string>();
  const [restrictedGroups, setRestrictedGroups] = useState<string[]>();
  const [reviewOpen, setReviewOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const entry = version.entry;
  const state = approvalState(version);
  const origin = keylessOrigin(entry);
  const recipients = deciderRecipients(entry);
  const readOnly = toolReplay(entry) === "read_only";
  const plugins = agentPlugins(entry);
  const installed = !!entry.installation;
  const decides =
    tenant !== null &&
    selected !== undefined &&
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
  const currentRestriction = version.approval?.restriction;
  const restricts = restricting ?? currentRestriction != null;
  const restrictionText =
    subjectNames ?? currentRestriction?.subjects.join(", ") ?? "";
  const restrictionSubjects = restrictionText
    .split(",")
    .map((name) => name.trim())
    .filter((name) => name.length > 0);
  const restrictionGroups =
    restrictedGroups ??
    currentRestriction?.groups.filter((group) => assignable.includes(group)) ??
    [];
  const requestedRestriction = (): Restriction | null | undefined => {
    if (!restricts) return currentRestriction != null ? null : undefined;
    if (restrictionSubjects.length === 0 && restrictionGroups.length === 0) {
      return currentRestriction != null ? null : undefined;
    }
    return { subjects: restrictionSubjects, groups: restrictionGroups };
  };
  const path = encodeURIComponent(tenant ?? "");
  const decide = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries();
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 400) {
        // A missing dependency needs the whole-closure review, not a retry.
        setReviewOpen(true);
      } else if (reason instanceof ApiError && reason.status === 409) {
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
        ...approvalRequest(version, true, sentGroups, requestedRestriction()),
        node: selected!.name,
      }),
    );
  const withdraw = () =>
    decide(() =>
      authorizationSetCatalog(path, {
        ...approvalRequest(version, false),
        node: selected!.name,
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
        {currentRestriction != null && (
          <span className="min-w-0 text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {t("catalogRestrictedTo")}: {restrictionLabel(currentRestriction)}
          </span>
        )}
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
      <SecretDisclosure entry={entry} />
      {recipients && <DeciderRecipients recipients={recipients} />}
      {entry.kind === "agent" && (
        <RegisteredCompactionDetails
          normalization={entry.binding_normalization}
          definitions={versions.map(({ entry }) => entry)}
        />
      )}
      {readOnly && decides && state !== "approved" && (
        <Notice tone="warning" role="status" className="read-only-notice">
          {t("catalogReadOnlyNotice")}
        </Notice>
      )}
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
      {decides && state !== "approved" && (
        <div
          className="grid gap-1.5"
          role="group"
          aria-label={t("catalogRestriction")}
        >
          <label className="flex items-center gap-2 text-xs font-semibold text-foreground">
            <input
              type="checkbox"
              className="size-3.5 shrink-0 accent-primary"
              disabled={busy}
              checked={restricts}
              onChange={(event) => {
                setRestricting(event.target.checked);
                if (!event.target.checked) {
                  setSubjectNames("");
                  setRestrictedGroups([]);
                }
              }}
            />
            {t("catalogRestriction")}
          </label>
          {restricts && (
            <>
              <label className="grid gap-1 text-xs">
                <span>{t("catalogRestrictionSubjects")}</span>
                <input
                  className="min-w-0 rounded-md border border-border bg-surface px-2 py-1 font-mono text-xs text-foreground"
                  disabled={busy}
                  value={restrictionText}
                  placeholder="alice, acme/agents/writer@1.0.0"
                  onChange={(event) => setSubjectNames(event.target.value)}
                />
              </label>
              {assignable.length > 0 && (
                <div
                  className="grid gap-1"
                  role="group"
                  aria-label={t("catalogRestrictionGroups")}
                >
                  <span className={subheading}>
                    {t("catalogRestrictionGroups")}
                  </span>
                  <GroupChoice
                    groups={assignable}
                    selected={restrictionGroups}
                    disabled={busy}
                    change={setRestrictedGroups}
                  />
                </div>
              )}
              <Hint>{t("catalogRestrictionHelp")}</Hint>
            </>
          )}
        </div>
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
      {tenant !== null &&
        selected !== undefined &&
        version.needs_approval &&
        !installed && (
          <div className="flex flex-wrap gap-2">
            {decides && state === "approved" ? (
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => void withdraw()}
              >
                {t("catalogWithdraw")}
              </Button>
            ) : decides ? (
              <Button size="sm" disabled={busy} onClick={() => void approve()}>
                {t("catalogApprove")}
              </Button>
            ) : null}
            <Button
              size="sm"
              variant={decides ? "outline" : "default"}
              disabled={busy}
              onClick={() => setReviewOpen((open) => !open)}
            >
              {t("approvalReview")}
            </Button>
          </div>
        )}
      {reviewOpen && tenant !== null && selected !== undefined && (
        <ApprovalReviewPanel
          tenant={tenant}
          node={selected.name}
          root={{ id: entry.id, version: entry.version }}
          subject={access.kind === "subject" ? access.subject : undefined}
          onClose={() => setReviewOpen(false)}
          onChanged={() =>
            client.invalidateQueries({ queryKey: ["registry-versions"] })
          }
        />
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
                  {record.restriction != null && (
                    <span className="col-span-2 min-w-0 text-[11px] text-muted-foreground [overflow-wrap:anywhere] sm:col-span-1">
                      {t("catalogRestrictedTo")}:{" "}
                      {restrictionLabel(record.restriction)}
                    </span>
                  )}
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
