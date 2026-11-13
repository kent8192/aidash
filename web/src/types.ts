// UI names for the server-owned, Orval-generated API contract.
export type {
  BTreeMap as Localized,
  EntityRef,
  Entry,
  Task,
  Workspace,
  RunSummary as Run,
  HumanRequest,
  Artifact,
  Conversation,
  Invocation,
  Discovery,
  Event as MeshEvent,
} from "./generated/models";
import type { StateResponse } from "./generated/models";

/** The Node a dashboard session works in: its address, which names the Node in
 *  every Run's Home and every Agent's subject, and the endpoint of the
 *  Deployment that hosts it. */
export interface WorkingNode {
  id: string;
  endpoint: string;
}

/** What the dashboard reads: the Tenant's State with the Node it works in. */
export type TenantState = StateResponse;
export type State = TenantState & { node: WorkingNode };
