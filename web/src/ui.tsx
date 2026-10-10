import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "./components/ui/dialog";
import { Badge as StatusBadge } from "./components/ui/badge";
import {
  createContext,
  cloneElement,
  useId,
  type ReactElement,
  useContext,
  type ReactNode,
} from "react";
import type { Entry, EntityRef, State, Discovery } from "./types";
import en from "./locales/en-US.json";
import ja from "./locales/ja-JP.json";
import { disambiguateLabels } from "./display-labels";
export type Locale = "en-US" | "ja-JP";
export const LocaleContext = createContext<Locale>("ja-JP");
export function useI18n() {
  const locale = useContext(LocaleContext);
  const dictionary: Record<string, string> = locale === "ja-JP" ? ja : en;
  const t = (key: string) => dictionary[key] ?? key;
  const local = (value: Record<string, string>) =>
    [
      value[locale],
      value[locale.slice(0, 2)],
      value.en,
      ...Object.values(value),
    ].find((name) => name?.trim()) ?? "";
  const entityName = (entry: { name: Record<string, string> }) =>
    local(entry.name) || t("unnamedEntity");
  const entityLabel = (entry: {
    name: Record<string, string>;
    version: string;
  }) => `${entityName(entry)} · ${entry.version}`;
  return { locale, t, local, entityName, entityLabel };
}
export function useEntryLabel(entries: readonly Entry[]) {
  const label = useEntityLabel(entries);
  const { t } = useI18n();
  return (reference: EntityRef) => {
    const entry = entries.find(
      (entry) =>
        entry.id === reference.id && entry.version === reference.version,
    );
    return entry
      ? label(entry)
      : `${t("unavailableEntity")} · ${reference.version}`;
  };
}

export function useEntityLabel(entries: readonly Entry[]) {
  const { entityLabel } = useI18n();
  const labels = disambiguateLabels(
    entries,
    (entry) => `${entry.id}@${entry.version}`,
    entityLabel,
  );
  return (entry: Entry) =>
    labels.get(`${entry.id}@${entry.version}`) ?? entityLabel(entry);
}

export function useEntityName(entries: readonly Entry[]) {
  const { entityName, entityLabel } = useI18n();
  const label = useEntityLabel(entries);
  return (entry: Entry) =>
    `${entityName(entry)}${label(entry).slice(entityLabel(entry).length)}`;
}

export function useAgentLabel(data: State, discovery?: Discovery) {
  const localLabel = useEntryLabel(data.registry);
  const { entityLabel, t } = useI18n();
  const agents = discovery?.agents ?? [];
  const remoteLabels = disambiguateLabels(
    agents,
    (agent) =>
      `${agent.node_id}/agents/${agent.entity.id}@${agent.entity.version}`,
    (agent) => entityLabel(agent.entity),
    (agent) => agent.node_id,
  );
  return (node: string, reference: EntityRef) => {
    if (node === data.node.id) return localLabel(reference);
    const agent = discovery?.agents.find(
      (agent) =>
        agent.node_id === node &&
        agent.entity.id === reference.id &&
        agent.entity.version === reference.version,
    );
    return agent
      ? (remoteLabels.get(
          `${node}/agents/${reference.id}@${reference.version}`,
        ) ?? entityLabel(agent.entity))
      : `${t("unavailableEntity")} · ${reference.version}`;
  };
}

export type StatusTone = "brand" | "success" | "warning" | "danger" | "neutral";
const statusTones: Record<string, StatusTone> = {
  RUNNING: "brand",
  THINKING: "brand",
  TOOL_CALL: "brand",
  ACTIVE: "brand",
  STARTED: "brand",
  QUEUED: "brand",
  PREPARING: "brand",
  COMMITTING: "brand",
  ENABLED: "brand",
  COMPLETED: "success",
  COMMITTED: "success",
  APPLIED: "success",
  PREPARED: "success",
  RESOLVED: "success",
  BLOCKED: "warning",
  WAITING: "warning",
  PAUSED: "warning",
  APPROVAL_REQUIRED: "warning",
  CONFIRMATION: "warning",
  QUESTION: "warning",
  INFORMATION_REQUEST: "warning",
  UNCERTAIN: "warning",
  PENDING_APPROVAL: "warning",
  PENDING: "warning",
  RELEASING: "warning",
  ABORTING: "warning",
  FAILED: "danger",
  DENIED: "danger",
  ABORTED: "danger",
  ERROR: "danger",
};
/** Semantic tone for a server status word (TaskStatus, RunPhase, transaction and request states). */
export function statusTone(value: string): StatusTone {
  return statusTones[value.toUpperCase()] ?? "neutral";
}
/** Status word badge; `tone` overrides `statusTone` for vocabularies it does not cover. */
export function Badge({ value, tone }: { value: string; tone?: StatusTone }) {
  const { t } = useI18n();
  return (
    <StatusBadge
      tone={tone ?? statusTone(value)}
      className={`badge ${value.toLowerCase()}`}
    >
      <span aria-hidden className="size-1.5 rounded-full bg-current" />
      {t(value)}
    </StatusBadge>
  );
}
export function JsonView({ value }: { value: unknown }) {
  return (
    <pre className="json max-h-96 overflow-auto rounded-md border border-border bg-background p-3 font-mono text-xs leading-relaxed text-muted-foreground">
      {typeof value === "string" ? value : JSON.stringify(value, null, 2)}
    </pre>
  );
}
export function Empty() {
  const { t } = useI18n();
  return (
    <div className="empty flex flex-col items-start gap-1 rounded-lg border border-dashed border-border-strong px-4 py-6">
      <h3 className="text-[13px] font-medium text-foreground">{t("empty")}</h3>
      <p className="max-w-[60ch] text-xs text-muted-foreground">
        {t("emptyHelp")}
      </p>
    </div>
  );
}
export function Field({
  label,
  children,
}: {
  label: string;
  children: ReactElement<{ id?: string }>;
}) {
  const generatedId = useId();
  const id = children.props.id ?? generatedId;
  return (
    <div className="field grid gap-1.5">
      <label htmlFor={id} className="text-xs font-medium text-muted-foreground">
        {label}
      </label>
      {cloneElement(children, { id })}
    </div>
  );
}
export function Modal({
  title,
  children,
  close,
}: {
  title: string;
  children: ReactNode;
  close: () => void;
}) {
  const { t } = useI18n();
  return (
    <Dialog
      open
      onOpenChange={(value) => {
        if (!value) close();
      }}
    >
      <DialogContent
        className="intent-dialog flex max-w-2xl flex-col overflow-hidden has-[.agent-graph]:w-[min(1120px,calc(100vw-2rem))] has-[.agent-graph]:max-w-[calc(100vw-2rem)]"
        aria-describedby={undefined}
        closeLabel={t("close")}
      >
        <DialogHeader className="shrink-0">
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <div className="-mx-5 grid min-h-0 min-w-0 flex-1 content-start gap-4 overflow-y-auto px-5 pb-1">
          {children}
        </div>
      </DialogContent>
    </Dialog>
  );
}
export function Panel({
  title,
  action,
  children,
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="panel grid min-w-0 gap-3 border-t border-border pt-4 first:border-t-0 first:pt-0">
      <div className="flex min-h-8 items-center justify-between gap-3">
        <h2 className="text-[13px] font-semibold text-foreground">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}
