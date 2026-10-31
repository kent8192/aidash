/** Ranges the server enforces for a Tenant's Subnode limits (#660). */
export const MEMBERS_PER_SUBNODE = { min: 1, max: 20 } as const;
export const SUBNODES_PER_RUN = { min: 1, max: 10 } as const;

export type SubnodeLimitsInput = {
  members_per_subnode: number;
  subnodes_per_run: number;
  remote_runs_may_create: boolean;
};

export type SubnodeLimitsProblem = "members_per_subnode" | "subnodes_per_run";

const whole = (value: number, range: { min: number; max: number }) =>
  Number.isInteger(value) && value >= range.min && value <= range.max;

/** The fields whose values the server would reject. */
export function subnodeLimitsProblems(
  input: SubnodeLimitsInput,
): SubnodeLimitsProblem[] {
  const problems: SubnodeLimitsProblem[] = [];
  if (!whole(input.members_per_subnode, MEMBERS_PER_SUBNODE))
    problems.push("members_per_subnode");
  if (!whole(input.subnodes_per_run, SUBNODES_PER_RUN))
    problems.push("subnodes_per_run");
  return problems;
}

/** A Subnode's title, or the head of its id when it has none. */
export function subnodeTitle(subnode: {
  id: string;
  title: string | null;
}): string {
  return subnode.title?.trim() || subnode.id.slice(0, 8);
}

export const subnodeActive = (subnode: { state: string }) =>
  subnode.state === "active";
