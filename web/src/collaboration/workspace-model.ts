import type { Run } from "../types";

export type LocatedRun = { run: Run; node: string };
export const terminalRun = (run: Run) =>
  ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
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
