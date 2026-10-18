import type { RegistryVersion, RequestApproval } from "./generated/models";
import type { EntityRef } from "./types";

/** Where one version of the Tenant's Registry stands for Catalog Approval. */
export type ApprovalState = "builtin" | "approved" | "withdrawn" | "unapproved";

export function approvalState(version: RegistryVersion): ApprovalState {
  if (!version.needs_approval) return "builtin";
  if (!version.approval) return "unapproved";
  return version.approval.enabled ? "approved" : "withdrawn";
}

const key = (reference: EntityRef) => `${reference.id}@${reference.version}`;

function reference(value: unknown): EntityRef | null {
  if (!value || typeof value !== "object") return null;
  const { id, version } = value as Record<string, unknown>;
  return typeof id === "string" && typeof version === "string"
    ? { id, version }
    : null;
}

/** The exact versions a definition names directly, as the server reads them
 *  when it refuses an approval whose dependency is unapproved. */
export function dependencies(entry: {
  kind: string;
  config: unknown;
}): EntityRef[] {
  const config = (entry.config ?? {}) as Record<string, unknown>;
  const list = (value: unknown): unknown[] =>
    Array.isArray(value) ? value : [];
  const field = (value: unknown, name: string): unknown =>
    value && typeof value === "object"
      ? (value as Record<string, unknown>)[name]
      : undefined;
  const named =
    entry.kind === "agent"
      ? [
          config.model,
          config.cluster,
          field(field(config.context_policy, "summary"), "model"),
          ...list(config.bindings).map((binding) => field(binding, "target")),
        ]
      : entry.kind === "bundle"
        ? list(config.members)
        : entry.kind === "cluster"
          ? [config.coordinator]
          : [];
  return named.map(reference).filter((value) => value !== null);
}

export type Closure = {
  /** The unapproved versions the target needs, nearest first. */
  dependencies: RegistryVersion[];
  /** Needed versions this request cannot approve: absent from the Tenant's
   *  Registry, or installed copies that Marketplace activation approves. */
  blocked: { reference: EntityRef; installed: boolean }[];
};

/** Everything an approval of `target` waits on. `named` is the dependency the
 *  server reported, kept even where this reading of the definition missed it. */
export function unapprovedClosure(
  target: RegistryVersion,
  versions: readonly RegistryVersion[],
  named?: EntityRef,
): Closure {
  const known = new Map(
    versions.map((version) => [key(version.entry), version]),
  );
  const seen = new Set([key(target.entry)]);
  const queue = [...dependencies(target.entry), ...(named ? [named] : [])];
  const closure: Closure = { dependencies: [], blocked: [] };
  for (let next = queue.shift(); next; next = queue.shift()) {
    if (seen.has(key(next))) continue;
    seen.add(key(next));
    const version = known.get(key(next));
    if (!version) {
      closure.blocked.push({ reference: next, installed: false });
      continue;
    }
    const state = approvalState(version);
    if (state === "approved" || state === "builtin") continue;
    if (version.entry.installation) {
      closure.blocked.push({ reference: next, installed: true });
      continue;
    }
    closure.dependencies.push(version);
    queue.push(...dependencies(version.entry));
  }
  return closure;
}

const UNAPPROVED = "CATALOG_DEPENDENCY_UNAPPROVED:";

/** The dependency a refused approval names, or `null` for any other error. */
export function unapprovedDependency(message: string): EntityRef | null {
  if (!message.startsWith(UNAPPROVED)) return null;
  const named = message.slice(UNAPPROVED.length).trim();
  const at = named.lastIndexOf("@");
  return at > 0
    ? { id: named.slice(0, at), version: named.slice(at + 1) }
    : null;
}

export function compareVersions(left: string, right: string): number {
  return left.localeCompare(right, "en", { numeric: true });
}

/** The groups of the subject of the latest approved earlier version of the
 *  same agent, which a new version's approval starts from. */
export function earlierAgentGroups(
  target: RegistryVersion,
  versions: readonly RegistryVersion[],
): string[] | undefined {
  let latest: RegistryVersion | undefined;
  for (const version of versions) {
    if (
      version.entry.kind !== "agent" ||
      version.entry.id !== target.entry.id ||
      !version.subject ||
      approvalState(version) !== "approved" ||
      compareVersions(version.entry.version, target.entry.version) >= 0
    )
      continue;
    if (
      !latest ||
      compareVersions(version.entry.version, latest.entry.version) > 0
    )
      latest = version;
  }
  return latest?.subject?.groups;
}

/** The groups the server gives a new agent subject when a request names none. */
export function defaultAgentGroups(assignable: readonly string[]): string[] {
  return assignable.includes("members") ? ["members"] : [];
}

/** What an approval sends as `groups`: nothing when the default applies, so
 *  the approval needs no `subject.group.update`. */
export function requestedGroups(
  chosen: readonly string[],
  assignable: readonly string[],
): string[] | undefined {
  const fallback = defaultAgentGroups(assignable);
  return chosen.length === fallback.length &&
    chosen.every((group) => fallback.includes(group))
    ? undefined
    : [...chosen].sort();
}

export function approvalRequest(
  version: RegistryVersion,
  enabled: boolean,
  groups?: string[],
): RequestApproval {
  return {
    entry: { id: version.entry.id, version: version.entry.version },
    enabled,
    expected_revision: version.approval?.revision ?? 0,
    ...(groups ? { groups } : {}),
  };
}
