import type { Run } from "../types";

export type LocatedRun = { run: Run; node: string };
export const terminalRun = (run: Run) =>
  ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
export const resumableRun = (
  run: Pick<Run, "recovery" | "state_error" | "phase" | "state">,
) =>
  !["invalidated", "context_budget", "durable_memory_budget"].includes(
    run.recovery?.semantic_reason ?? "",
  ) &&
  (!run.state_error ||
    (run.phase === "WAITING" &&
      run.state?.phase === "WAITING" &&
      run.state.data.reason === "failure_delivery"));
/**
 * Whether a person can abandon the task: a failed, blocked or cancelled one,
 * or an open one nobody has started. The server judges "started" under the
 * task's lock, so a Run or handover the page cannot see yet is still refused.
 */
export const abandonable = (
  task: { status: string },
  taskRuns: readonly unknown[],
) =>
  ["FAILED", "BLOCKED", "CANCELLED"].includes(task.status) ||
  (task.status === "OPEN" && taskRuns.length === 0);
export function channelAgents(runs: LocatedRun[]) {
  const agents = new Map<string, LocatedRun>();
  for (const item of [...runs].sort((a, b) =>
    b.run.updated_at.localeCompare(a.run.updated_at),
  )) {
    const key = JSON.stringify([
      item.node,
      item.run.agent_id,
      item.run.agent_version,
    ]);
    const previous = agents.get(key);
    if (!previous || (terminalRun(previous.run) && !terminalRun(item.run)))
      agents.set(key, item);
  }
  return [...agents.values()];
}
