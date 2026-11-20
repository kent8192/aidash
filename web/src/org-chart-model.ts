/** The org chart and flow stop of #1012 (ADR 0143). The types follow the
 *  server's `OrgChart` and `FlowStopped`; they are written by hand until the
 *  OpenAPI client is regenerated with these routes. */
export type EntityRefLike = { id: string; version: string };

export type Spend = { nanocredits: number; unpriced_calls: number };

export type NodeCreator =
  | { kind: "person"; identity?: string }
  | { kind: "deployment" }
  | { kind: "agent"; root_subject: string; task_id: string; run_id: string };

export type ChartDuty = {
  id: string;
  title: string;
  requester: string;
  agent: EntityRefLike;
  state: string;
  created_by_task?: string;
  flow?: string;
  spend?: Spend;
};

export type ChartSubnode = {
  id: string;
  title?: string;
  creator_agent: EntityRefLike;
  creator_task: string;
  root_subject: string;
  members: EntityRefLike[];
  flow?: string;
};

export type ChartNode = {
  name: string;
  label: string;
  default: boolean;
  created_by: NodeCreator;
  flow?: string;
  duties: ChartDuty[];
  /** Duties here the person may not see. */
  hidden_duties: number;
  subnodes: ChartSubnode[];
  /** Active Subnodes here the person may not see. */
  hidden_subnodes: number;
  spend?: Spend;
};

export type OrgChart = {
  since: string;
  nodes: ChartNode[];
  people: { subject: string; spend: Spend }[];
};

export type StopOutcome = "stopped" | "already_stopped" | "failed";
export type StoppedItem = { id: string; outcome: StopOutcome; error?: string };

export type FlowStopped = {
  root_task: string;
  tasks: number;
  runs: StoppedItem[];
  /** Work handed to other Deployments, by Remote Grant. */
  remote: StoppedItem[];
  subnodes: StoppedItem[];
  duties: StoppedItem[];
  nodes: { name: string; label: string }[];
};

/** The org chart route of a Tenant. */
export function orgChartPath(tenant: string): string {
  return `/api/tenants/${encodeURIComponent(tenant)}/org-chart`;
}

/** The stop route of the flow whose Root Task is `rootTask`. */
export function flowStopPath(tenant: string, rootTask: string): string {
  return `/api/tenants/${encodeURIComponent(tenant)}/flows/${encodeURIComponent(rootTask)}/stop`;
}

/** Nanocredits as credits with two decimals, as Budgets show them. */
export function spendText(spend: Spend): string {
  const credits = (spend.nanocredits / 1e9).toFixed(2);
  return spend.unpriced_calls > 0
    ? `${credits} (+${spend.unpriced_calls})`
    : credits;
}

/** The Root Tasks of the flows that made something on this Node, each once,
 *  in the order the chart shows them. */
export function nodeFlows(node: ChartNode): string[] {
  const flows = [
    node.flow,
    ...node.duties.map((duty) => duty.flow),
    ...node.subnodes.map((subnode) => subnode.flow),
  ].filter((flow): flow is string => typeof flow === "string");
  return [...new Set(flows)];
}

/** How many things a stop stopped, left alone and failed to stop, and the
 *  Nodes it left in place. */
export function stopSummary(result: FlowStopped): {
  stopped: number;
  already: number;
  failed: number;
  nodes: number;
} {
  const items = [
    ...result.runs,
    ...result.remote,
    ...result.subnodes,
    ...result.duties,
  ];
  return {
    stopped: items.filter((item) => item.outcome === "stopped").length,
    already: items.filter((item) => item.outcome === "already_stopped").length,
    failed: items.filter((item) => item.outcome === "failed").length,
    nodes: result.nodes.length,
  };
}

/** How many Duties and Subnodes on a Node act for others and stay hidden. */
export function hiddenCount(node: ChartNode): number {
  return node.hidden_duties + node.hidden_subnodes;
}
