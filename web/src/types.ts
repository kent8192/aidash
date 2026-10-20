// UI names for the server-owned, Orval-generated API contract.
export type {
  BTreeMap as Localized,
  EntityRef,
  Entry,
  Task,
  Workspace,
  RunInspection as Run,
  HumanRequest,
  Artifact,
  Conversation,
  Invocation,
  Discovery,
  Event as MeshEvent,
  StateResponse as State,
} from "./generated/models";
import type {
  HumanRequest,
  Invocation,
  RunInspection,
} from "./generated/models";

/** What another Node reports about the work it is home to. */
export interface MeshNode {
  node_id: string;
  runs: RunInspection[];
  human_requests: HumanRequest[];
  invocations: Invocation[];
}
export interface PeerError {
  node_id: string;
  error: string;
}
/**
 * Other Nodes' observations. No Deployment-wide view of them exists any more
 * (#241): a Node is a Tenant's, so the dashboard has none to show yet.
 */
export interface Mesh {
  nodes: MeshNode[];
  errors: PeerError[];
}
