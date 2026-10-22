import type { NodeRecord } from "./generated/models";
import type { Run, State, TenantState } from "./types";

/** What the dashboard knows about one Node of the signed-in Tenant. */
export type TenantNode = Pick<
  NodeRecord,
  "name" | "address" | "label" | "default"
>;

export const NODE_STORAGE_PREFIX = "aidash-node:";

/** Where a browser remembers the Node it selected in one Tenant. */
export function nodeStorageKey(tenant: string): string {
  return `${NODE_STORAGE_PREFIX}${tenant}`;
}

/**
 * The Node the dashboard shows: the remembered one while the Tenant still
 * holds it, otherwise the default Node. A Tenant always has a default Node, so
 * only an empty list, a Deployment Administrator or a failed read has none.
 */
export function chooseNode<Node extends Pick<TenantNode, "name" | "default">>(
  nodes: readonly Node[],
  remembered: string | null,
): Node | undefined {
  return (
    nodes.find((node) => node.name === remembered) ??
    nodes.find((node) => node.default) ??
    nodes[0]
  );
}

/**
 * What one Node of a Tenant holds. State lists every Node of the Tenant, but a
 * person works in one at a time: its Workspaces and the Tasks, Runs, requests,
 * Conversations and Artifacts beneath them. `node.id` becomes the Node's
 * address, because every comparison of a Run's Home with the local Node means
 * that Node. The Registry belongs to the Tenant and stays whole.
 *
 * Without a Node (a Deployment Administrator has no Tenant) nothing is
 * filtered, and `node.id` is the Deployment's.
 */
export function nodeView(
  state: TenantState,
  node: Pick<TenantNode, "name" | "address"> | undefined,
): State {
  const { id: deployment, endpoint } = state.deployment;
  if (!node) return { ...state, node: { id: deployment, endpoint } };
  const workspaces = state.workspaces.filter(
    (workspace) => workspace.node === node.name,
  );
  const ids = new Set(workspaces.map((workspace) => workspace.id));
  return {
    ...state,
    node: { id: node.address, endpoint },
    workspaces,
    tasks: state.tasks.filter((task) => ids.has(task.workspace_id)),
    runs: state.runs.filter((run) => ids.has(run.workspace_id)),
    human_requests: state.human_requests.filter((request) =>
      ids.has(request.workspace_id),
    ),
    conversations: state.conversations.filter((conversation) =>
      ids.has(conversation.workspace_id),
    ),
    events: state.events.filter(
      (event) => event.node === null || event.node === node.name,
    ),
    artifacts: state.artifacts.filter((artifact) =>
      ids.has(artifact.workspace_id),
    ),
  };
}

/** The name a Node goes by in the dashboard: its label, or its name when the
 *  Node is not in the list (yet). */
export function nodeLabel(
  nodes: readonly Pick<TenantNode, "name" | "label">[],
  name: string,
): string {
  return nodes.find((node) => node.name === name)?.label ?? name;
}

/** `aidash://<deployment>/<node name>` split into its two parts. */
export function parseNodeAddress(
  address: string,
): { deployment: string; node: string } | null {
  const match = /^aidash:\/\/([^/\s]+)\/([^/\s]+)$/.exec(address.trim());
  return match ? { deployment: match[1], node: match[2] } : null;
}

/** `aidash://<deployment>` of an address that names a Deployment or one of its Nodes. */
export function deploymentId(address: string): string {
  return address.replace(/^(aidash:\/\/[^/]*).*$/, "$1");
}

/** Monitoring locality: a Subject's selected Node, or a Deployment
 *  Administrator's own Deployment. This does not grant execution authority. */
export function isLocalRun(
  run: Pick<Run, "home_node">,
  state: Pick<State, "access" | "node">,
): boolean {
  if (state.access.kind === "subject") return run.home_node === state.node.id;
  const home = parseNodeAddress(run.home_node);
  return (
    home !== null &&
    `aidash://${home.deployment}` === deploymentId(state.node.id)
  );
}

/**
 * The Deployment whose Registry `state` lists. A definition names it
 * (`registry_deployment`, an agent tool's `node_id`), while `node.id` is a
 * Node's address once a Tenant's Node is selected.
 */
export function registryDeployment(state: { node: { id: string } }): string {
  return deploymentId(state.node.id);
}
