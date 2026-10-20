/**
 * A person's Approval Mode and Approval Rules as the dashboard presents them
 * (ADR 0076). The server stays the authority: it refuses a mode the Tenant's
 * policy does not allow and a rule target of the wrong shape, so this module
 * only shapes what is sent and decides what to offer.
 */
import type {
  ApprovalKind,
  ApprovalMode,
  ApprovalModeChoice,
  RequestApprovalPreferencesUpdate,
  RequestApprovalRuleInput,
  RequestRuleTarget,
  RequestThreadApprovalModeUpdate,
  RuleEffect,
  RuleTarget,
} from "./generated/models";

/** The modes in the order the dashboard offers them. */
export const approvalModes: readonly ApprovalMode[] = [
  "manual",
  "auto",
  "dont_ask",
  "bypass",
];

export const approvalKinds: readonly ApprovalKind[] = [
  "tool_call",
  "outbound",
  "web_search",
  "page_reading",
  "generation",
];

export const ruleEffects: readonly RuleEffect[] = ["allow", "ask", "deny"];

/** What a rule target of each kind looks like. */
export const targetExamples: Record<ApprovalKind, string> = {
  tool_call: "tool:crm#invoke",
  outbound: "https://example.com",
  web_search: "research.brave@1",
  page_reading: "example.com",
  generation: "reviewers",
};

/** Whether the Tenant's policy lets the person use `mode`. */
export function modeAllowed(
  choices: readonly ApprovalModeChoice[],
  mode: ApprovalMode,
): boolean {
  return choices.some((choice) => choice.mode === mode && choice.allowed);
}

type Acceptance = { bypass_accepted_at: string | null };

/** Choosing Bypass before the person has accepted its risk asks them first. */
export function bypassNeedsConfirmation(
  preferences: Acceptance,
  mode: ApprovalMode,
): boolean {
  return mode === "bypass" && preferences.bypass_accepted_at === null;
}

/**
 * The body that makes `mode` the person's default. The server replaces the
 * whole row, so the Answer Deadline is echoed back instead of cleared.
 * `confirmedBypass` is sent only after the person read what Bypass does.
 */
export function preferencesUpdate(
  preferences: {
    revision: number;
    answer_deadline_seconds: number | null;
  },
  mode: ApprovalMode,
  confirmedBypass: boolean,
): RequestApprovalPreferencesUpdate {
  return {
    expected_revision: preferences.revision,
    default_mode: mode,
    answer_deadline_seconds: preferences.answer_deadline_seconds,
    accept_bypass: mode === "bypass" && confirmedBypass,
  };
}

export type RuleDraft = {
  kind: ApprovalKind;
  every: boolean;
  target: string;
  effect: RuleEffect;
};

export type RuleInputResult =
  | { ok: true; input: RequestApprovalRuleInput }
  | { ok: false; problem: "target_required" };

/**
 * The rule a draft asks for: every target of the kind, or exactly one. The
 * server checks that an exact target has the shape its kind compares with.
 */
export function ruleInput(draft: RuleDraft): RuleInputResult {
  const target = draft.target.trim();
  if (!draft.every && target === "")
    return { ok: false, problem: "target_required" };
  const named: RequestRuleTarget = draft.every ? "every" : { exact: target };
  return {
    ok: true,
    input: { kind: draft.kind, effect: draft.effect, target: named },
  };
}

/** The exact target a rule names, or null when it covers every target of its kind. */
export function ruleTargetText(target: RuleTarget): string | null {
  return target === "every" ? null : target.exact;
}

/** What gave an approval in advance, as the approval card records it. */
export type GivenBy = { by: "rule"; rule: string } | { by: "bypass" };

export function givenBy(value: unknown): GivenBy | null {
  if (!value || typeof value !== "object") return null;
  const { rule, mode } = value as Record<string, unknown>;
  if (mode === "bypass") return { by: "bypass" };
  if (typeof rule === "string" && rule !== "") return { by: "rule", rule };
  return null;
}

/** Why an option of the Thread switcher cannot be chosen. */
export type Unavailable = "policy" | "bypass_unaccepted";

export type ThreadModeOption = {
  /** `default` clears the Thread's own mode. */
  value: "default" | ApprovalMode;
  mode: ApprovalMode;
  unavailable: Unavailable | null;
};

export type ThreadModeView = {
  mode: ApprovalMode | null;
  default_mode: ApprovalMode;
  modes: readonly ApprovalModeChoice[];
};

/**
 * The options of a Thread's mode switcher and the one in effect. The server
 * refuses Bypass until the person accepted its risk in their preferences, so
 * it stays unavailable while that is unknown. The option already in effect is
 * always listed, even when the ceiling has since lowered.
 */
export function threadModeOptions(
  view: ThreadModeView,
  bypassAccepted: boolean,
): { selected: "default" | ApprovalMode; options: ThreadModeOption[] } {
  const unavailable = (mode: ApprovalMode): Unavailable | null =>
    !modeAllowed(view.modes, mode)
      ? "policy"
      : mode === "bypass" && !bypassAccepted
        ? "bypass_unaccepted"
        : null;
  return {
    selected: view.mode ?? "default",
    options: [
      { value: "default", mode: view.default_mode, unavailable: null },
      ...approvalModes.map((mode) => ({
        value: mode,
        mode,
        unavailable: unavailable(mode),
      })),
    ],
  };
}

/** The body that sets, or with `default` clears, a Thread's mode. */
export function threadModeUpdate(
  value: "default" | ApprovalMode,
): RequestThreadApprovalModeUpdate {
  return { mode: value === "default" ? null : value };
}

export type DecisionChoice =
  | "allow_once"
  | "allow_run"
  | "allow_always"
  | "deny";

/**
 * The scope a decision carries. Only a Run grant has one: the server refuses
 * "Allow once" and "Always allow" with a scope, even when the person ticked the
 * Run-grant box before choosing "Always allow".
 */
export function decisionScope(
  choice: DecisionChoice,
  grant: { targets: readonly string[]; minutes: number; now: number },
): { targets: string[] | null; expires_at: string | null } {
  if (choice !== "allow_run") return { targets: null, expires_at: null };
  return {
    targets: [...grant.targets],
    expires_at: new Date(grant.now + grant.minutes * 60000).toISOString(),
  };
}
