import {
  createContext,
  cloneElement,
  useId,
  type ReactElement,
  useContext,
  useEffect,
  useRef,
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

export function Badge({ value }: { value: string }) {
  const { t } = useI18n();
  return <span className={`badge ${value.toLowerCase()}`}>{t(value)}</span>;
}
export function JsonView({ value }: { value: unknown }) {
  return (
    <pre className="json">
      {typeof value === "string" ? value : JSON.stringify(value, null, 2)}
    </pre>
  );
}
export function Empty() {
  const { t } = useI18n();
  return (
    <div className="empty">
      <span className="empty-mark">◇</span>
      <h3>{t("empty")}</h3>
      <p>{t("emptyHelp")}</p>
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
    <div className="field">
      <label htmlFor={id}>{label}</label>
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
  const ref = useRef<HTMLDialogElement>(null);
  const { t } = useI18n();
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  return (
    <dialog ref={ref} onCancel={close}>
      <div className="modal-heading">
        <h2>{title}</h2>
        <button className="icon-button" onClick={close} aria-label={t("close")}>
          ×
        </button>
      </div>
      <div className="modal-body">{children}</div>
    </dialog>
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
    <section className="panel">
      <div className="panel-heading">
        <h2>{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}
