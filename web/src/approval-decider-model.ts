/**
 * The approval Decider that judges the Tenant's Auto Approval Mode (ADR 0077).
 * The server stays the authority: it refuses a Decider the Tenant's Registry
 * does not hold as one of hook `approval`, so this module only decides what to
 * offer and shapes what is sent.
 */
import type {
  RegistryVersion,
  RequestApprovalDeciderUpdate,
} from "./generated/models";

type Reference = { id: string; version: string };

/** The `value` of the select option that resets to the built-in Decider. */
export const BUILT_IN_CHOICE = "";

/** How one Registry version is named in a select value and a settings link. */
export function versionKey(reference: Reference): string {
  return `${reference.id}@${reference.version}`;
}

function approvalHook(entry: RegistryVersion["entry"]): boolean {
  const config: unknown = entry.config;
  return (
    entry.kind === "decider" &&
    !!config &&
    typeof config === "object" &&
    (config as { hook?: unknown }).hook === "approval"
  );
}

/**
 * The Deciders a Tenant can name: its own definitions of hook `approval`.
 * A Deployment definition, which carries a reserved identifier prefix
 * (ADR 0033), is the built-in one and is chosen by resetting, not by name.
 */
export function approvalDeciderChoices(
  versions: readonly RegistryVersion[],
): RegistryVersion[] {
  return versions.filter(
    ({ entry }) =>
      approvalHook(entry) &&
      !entry.id.startsWith("aidash.") &&
      !entry.id.startsWith("core."),
  );
}

/** The Registry version a Decider reference names, when the Registry holds it. */
export function registryVersionOf(
  versions: readonly RegistryVersion[],
  reference: Reference,
): RegistryVersion | null {
  return (
    versions.find(
      ({ entry }) =>
        entry.id === reference.id && entry.version === reference.version,
    ) ?? null
  );
}

/**
 * The body that names the Decider a select value stands for, or with
 * {@link BUILT_IN_CHOICE} resets to the built-in one. The revision fences the
 * change against another administrator's.
 */
export function deciderUpdate(
  current: { revision: number },
  choice: string,
): RequestApprovalDeciderUpdate {
  const at = choice.lastIndexOf("@");
  return {
    expected_revision: current.revision,
    decider:
      choice === BUILT_IN_CHOICE || at < 1
        ? null
        : { id: choice.slice(0, at), version: choice.slice(at + 1) },
  };
}

/**
 * The Registry version a settings link asks to open, when `focus` names one.
 * Anything else, such as the semantic view's marker, names none.
 */
export function focusedVersion(
  versions: readonly RegistryVersion[],
  focus: string,
): RegistryVersion | null {
  return versions.find(({ entry }) => versionKey(entry) === focus) ?? null;
}
