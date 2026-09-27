/** UI projections only. Server policy remains the authority for every operation. */
export const primaryDestinations = ["collaboration", "graph"] as const;
export const settingsSections = [
  "agents",
  "workingFiles",
  "registry",
  "clusters",
  "generation",
  "authorization",
  "semantic",
  "transactions",
  "deployment",
  "marketplace",
  "node",
] as const;
export type Destination =
  | "collaboration"
  | "graph"
  | "creator"
  | "trust"
  | "settings";
export type SettingsSection = (typeof settingsSections)[number];
export type Location = {
  section: Destination;
  settings: SettingsSection;
  channel: string;
  focus: string;
  legacy: boolean;
};
export function parseQuery(search: string): Record<string, string> {
  return Object.fromEntries(new URLSearchParams(search));
}
export function stringifyQuery(search: Record<string, unknown>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(search)) {
    if (typeof value === "string" && value) params.set(key, value);
  }
  return params.size ? `?${params}` : "";
}
export function resolveLocation(pathname: string, search = ""): Location {
  const path = pathname.replace(/\/+$/, "") || "/";
  const name = path.split("/")[1] || "collaboration";
  const params = parseQuery(search);
  const settings = settingsSections.includes(params.view as SettingsSection)
    ? (params.view as SettingsSection)
    : "node";
  const common = {
    settings,
    channel: params.channel || "",
    focus: params.focus || "",
    legacy: false,
  };
  if (
    name === "collaboration" ||
    name === "graph" ||
    name === "creator" ||
    name === "trust" ||
    name === "settings"
  ) {
    return { ...common, section: name };
  }
  if (name === "mesh") return { ...common, section: "graph", legacy: true };
  if (settingsSections.includes(name as SettingsSection)) {
    return {
      ...common,
      section: "settings",
      settings: name as SettingsSection,
      legacy: true,
    };
  }
  return { ...common, section: "collaboration", legacy: true };
}
export function destination(
  section: Destination,
  context: { channel?: string; focus?: string; settings?: string } = {},
): string {
  return `/${section}${stringifyQuery({
    channel: context.channel,
    focus:
      section === "graph" || section === "creator" || section === "trust"
        ? context.focus
        : undefined,
    view:
      section === "settings" &&
      settingsSections.includes(context.settings as SettingsSection)
        ? context.settings
        : undefined,
  })}`;
}
export type WorkspaceRef = { id: string; title: string };
export type TaskRef = {
  id: string;
  workspace_id: string;
  status: string;
  created_by?: string;
  owner?: string | null;
};
export type RunRef = {
  id: string;
  workspace_id: string;
  task_id: string;
  agent_id: string;
  agent_version: string;
  home_node?: string;
};
export type EntryRef = {
  id: string;
  version: string;
  kind: string;
  config: Record<string, unknown>;
};
export type SelectedNode = {
  kind: string;
  available: boolean;
  entity?: { id: string; version: string };
  resourceId?: string;
};
function sameReference(
  value: unknown,
  ref: { id: string; version: string },
): boolean {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return record.id === ref.id && record.version === ref.version;
}
function configuredFor(entry: EntryRef, node: SelectedNode): boolean {
  if (!node.entity) return false;
  if (node.kind === "agent")
    return entry.id === node.entity.id && entry.version === node.entity.version;
  if (node.kind === "model" || node.kind === "cluster")
    return sameReference(entry.config[node.kind], node.entity);
  if (node.kind === "tool" || node.kind === "skill") {
    const refs = entry.config[`${node.kind}s`];
    return (
      Array.isArray(refs) &&
      refs.some((ref) => sameReference(ref, node.entity!))
    );
  }
  return false;
}
/** Only inspect the currently authorized local snapshot. Never infer links from text. */
export function relatedChannels(
  node: SelectedNode,
  data: {
    nodeId: string;
    workspaces: readonly WorkspaceRef[];
    tasks: readonly TaskRef[];
    runs: readonly RunRef[];
    registry: readonly EntryRef[];
  },
  current = "",
): WorkspaceRef[] {
  if (!node.available) return [];
  const ids = new Set<string>();
  if (node.kind === "task") {
    const task = data.tasks.find((task) => task.id === node.resourceId);
    if (task) ids.add(task.workspace_id);
  } else if (node.kind === "run") {
    const run = data.runs.find((run) => run.id === node.resourceId);
    if (run && (!run.home_node || run.home_node === data.nodeId))
      ids.add(run.workspace_id);
  } else {
    const cluster =
      node.kind === "cluster" && node.entity
        ? data.registry.find(
            (entry) =>
              entry.kind === "cluster" &&
              entry.id === node.entity!.id &&
              entry.version === node.entity!.version,
          )
        : undefined;
    const agents = data.registry.filter(
      (entry) =>
        entry.kind === "agent" &&
        (configuredFor(entry, node) ||
          (cluster && sameReference(cluster.config.coordinator, entry))),
    );
    for (const agent of agents) {
      for (const run of data.runs) {
        if (
          (!run.home_node || run.home_node === data.nodeId) &&
          run.agent_id === agent.id &&
          run.agent_version === agent.version
        )
          ids.add(run.workspace_id);
      }
      const qualified = `${data.nodeId}/agents/${agent.id}@${agent.version}`;
      for (const task of data.tasks) {
        if (task.owner === qualified || task.created_by === qualified)
          ids.add(task.workspace_id);
      }
    }
  }
  return data.workspaces
    .filter((workspace) => ids.has(workspace.id))
    .sort((a, b) => Number(b.id === current) - Number(a.id === current));
}
export function chooseChannel(
  channels: readonly WorkspaceRef[],
  requested: string,
): WorkspaceRef | undefined {
  return requested
    ? channels.find((channel) => channel.id === requested)
    : channels[0];
}
export function taskProgress(
  tasks: readonly TaskRef[],
  channel: string,
): { total: number; completed: number; active: number; attention: number } {
  const scoped = tasks.filter((task) => task.workspace_id === channel);
  return {
    total: scoped.length,
    completed: scoped.filter((task) => task.status === "COMPLETED").length,
    active: scoped.filter((task) =>
      ["CLAIMED", "RUNNING"].includes(task.status),
    ).length,
    attention: scoped.filter((task) =>
      ["BLOCKED", "FAILED"].includes(task.status),
    ).length,
  };
}
export function senderLabel(sender: string): {
  kind: "agent" | "human";
  name: string;
} {
  const marker = sender.indexOf("/agents/");
  return sender.startsWith("aidash://") && marker > "aidash://".length
    ? { kind: "agent", name: sender.slice(marker + "/agents/".length) }
    : { kind: "human", name: sender };
}
