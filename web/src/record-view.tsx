import { createContext, useContext, type ReactNode } from "react";
import type { State } from "./types";
import { JsonView, useI18n } from "./ui";
import { presentRecord } from "./record-presentation";

export const DisplayState = createContext<State | undefined>(undefined);
export function DisplayProvider({
  data,
  children,
}: {
  data?: State;
  children: ReactNode;
}) {
  return <DisplayState.Provider value={data}>{children}</DisplayState.Provider>;
}
export function useRecordLabels() {
  const data = useContext(DisplayState);
  const { entityName, entityLabel, t } = useI18n();
  const labels = new Map<string, string>();
  for (const entry of data?.registry ?? []) {
    labels.set(entry.id, entityName(entry));
    labels.set(`${entry.id}@${entry.version}`, entityLabel(entry));
    if (entry.kind === "agent")
      labels.set(
        `${data!.node.id}/agents/${entry.id}@${entry.version}`,
        entityLabel(entry),
      );
  }
  for (const workspace of data?.workspaces ?? [])
    labels.set(workspace.id, workspace.title || t("workspace"));
  for (const task of data?.tasks ?? [])
    labels.set(task.id, task.title || t("task"));
  for (const run of data?.runs ?? [])
    labels.set(
      run.id,
      `${labels.get(run.task_id) ?? t("execution")} · ${t(run.phase)}`,
    );
  for (const artifact of data?.artifacts ?? [])
    labels.set(artifact.id, artifact.name || t("artifact"));
  const endpointLabel = (endpoint: string) => {
    try {
      const url = new URL(endpoint);
      return `${url.host}${url.pathname === "/" ? "" : url.pathname}`;
    } catch {
      return t("unavailableEntity");
    }
  };
  if (data) labels.set(data.node.id, endpointLabel(data.node.endpoint));
  for (const peer of data?.peers ?? [])
    labels.set(peer.node_id, endpointLabel(peer.endpoint));
  return labels;
}
export function useNodeLabel() {
  const labels = useRecordLabels();
  const { t } = useI18n();
  return (id: string) => labels.get(id) ?? t("unavailableEntity");
}
export function RecordView({
  value,
  labels: extra,
}: {
  value: unknown;
  labels?: ReadonlyMap<string, string>;
}) {
  const labels = useRecordLabels();
  const { t } = useI18n();
  for (const [id, name] of extra ?? []) labels.set(id, name);
  return (
    <JsonView value={presentRecord(value, labels, t("unavailableEntity"))} />
  );
}

export function ReferenceName({ id }: { id: string }) {
  const labels = useRecordLabels();
  const { t } = useI18n();
  return <>{labels.get(id) ?? t("unavailableEntity")}</>;
}

export function PeerSelect({
  id,
  name,
  initial = "",
  disabled = false,
}: {
  id?: string;
  name: string;
  initial?: string;
  disabled?: boolean;
}) {
  const data = useContext(DisplayState);
  const { t } = useI18n();
  return (
    <select
      id={id}
      name={name}
      required
      defaultValue={initial}
      disabled={disabled}
    >
      <option value="">{t("choose")}</option>
      {data?.peers.map((peer) => (
        <option key={peer.node_id} value={peer.node_id}>
          <ReferenceName id={peer.node_id} />
        </option>
      ))}
      {initial && !data?.peers.some((peer) => peer.node_id === initial) && (
        <option value={initial}>{t("unavailableEntity")}</option>
      )}
    </select>
  );
}
