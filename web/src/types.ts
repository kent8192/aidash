export type Localized = Record<string, string>;
export type EntityRef = { id: string; version: string };
export type Entry = EntityRef & {
  kind: string;
  name: Localized;
  description: Localized;
  capabilities: string[];
  tags: string[];
  languages: string[];
  skills: string[];
  schema: Record<string, unknown>;
  config: Record<string, unknown>;
};
export type Task = {
  id: string;
  workspace_id: string;
  title: string;
  description: string;
  status: string;
  owner: string | null;
  revision: number;
  requirements: Record<string, unknown>;
  parent_id: string | null;
  dependencies: string[];
  created_by: string;
};
export type Workspace = {
  id: string;
  title: string;
  goal: string;
  state: Record<string, unknown>;
  revision: number;
};
export type Run = {
  id: string;
  task_id: string;
  workspace_id: string;
  home_node: string;
  agent_id: string;
  agent_version: string;
  phase: string;
  control: string;
  step: number;
  error: string | null;
  context: {
    usage?: {
      input_tokens?: number;
      output_tokens?: number;
      context_window?: number;
      compactions?: number;
    };
    history?: unknown[];
  };
  updated_at: string;
};
export type HumanRequest = {
  id: string;
  workspace_id: string;
  run_id: string;
  kind: string;
  prompt: string;
  response: unknown | null;
  created_at: string;
};
export type MeshEvent = {
  id: string;
  sequence: number;
  node_id: string;
  workspace_id: string | null;
  kind: string;
  data: unknown;
  created_at: string;
};
export type Artifact = {
  id: string;
  workspace_id: string;
  task_id: string;
  name: string;
  kind: string;
  content: unknown;
  created_by: string;
};
export type Conversation = {
  id: string;
  workspace_id: string;
  target: string;
  target_kind: string;
};
export type Peer = {
  node_id: string;
  endpoint: string;
  protocol_version: string;
  enabled: boolean;
  credential_env: string;
};
export type State = {
  node: { id: string; endpoint: string; protocol_version: string };
  registry: Entry[];
  workspaces: Workspace[];
  tasks: Task[];
  runs: Run[];
  human_requests: HumanRequest[];
  conversations: Conversation[];
  peers: Peer[];
  events: MeshEvent[];
  artifacts: Artifact[];
  installations: (EntityRef & { digest: string })[];
};
export type Invocation = {
  idempotency_key: string;
  run_id: string;
  tool: string;
  input: unknown;
  status: string;
  result: unknown;
  replay_safe: boolean;
};
export type Mesh = {
  nodes: {
    node_id: string;
    runs: Run[];
    human_requests: HumanRequest[];
    invocations: Invocation[];
  }[];
  errors: { node_id: string; error: string }[];
};
export type Discovery = {
  agents: { node_id: string; entity: Entry }[];
  errors: { node_id: string; error: string }[];
};
export type Package = {
  id: string;
  version: string;
  digest: string;
  manifest: {
    entity: Entry;
    author: string;
    permissions: string[];
    dependencies: EntityRef[];
  };
};
