/** A Tenant's Subnode settings (#660). Subnodes nest and are not capped per
 *  Run or in members (#1012); only the consent to Subnodes that work from
 *  another Node creates remains. */
export type SubnodeLimitsInput = {
  remote_runs_may_create: boolean;
};

/** A Subnode's title, or the head of its id when it has none. */
export function subnodeTitle(subnode: {
  id: string;
  title: string | null;
}): string {
  return subnode.title?.trim() || subnode.id.slice(0, 8);
}

export const subnodeActive = (subnode: { state: string }) =>
  subnode.state === "active";
