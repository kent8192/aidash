/**
 * A person's Approval Mode and Approval Rules as the dashboard presents them
 * (ADR 0076, ADR 0077). The server stays the authority: it refuses a mode the
 * Tenant's policy or Node does not allow and a rule target of the wrong shape,
 * so this module only shapes what is sent and decides what to offer.
 */
import type {
  ApprovalKind,
  ApprovalMode,
  ApprovalModeChoice,
  ApprovalModeReason,
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

/** Whether the Tenant's policy and this Node let the person use `mode`. */
export function modeAllowed(
  choices: readonly ApprovalModeChoice[],
  mode: ApprovalMode,
): boolean {
  return choices.some((choice) => choice.mode === mode && choice.allowed);
}

/**
 * What stands in the way of `mode`, as the server reports it. Bypass can be
 * allowed and still carry `not_accepted`: the person accepts its risk when
 * they first choose it as their default, so Settings keeps it selectable while
 * a Thread cannot use it yet.
 */
export function modeReason(
  choices: readonly ApprovalModeChoice[],
  mode: ApprovalMode,
): ApprovalModeReason | null {
  return choices.find((choice) => choice.mode === mode)?.reason ?? null;
}

/** The reasons that name the Tenant's approval Decider. */
export function needsApprovalDecider(
  choices: readonly ApprovalModeChoice[],
): boolean {
  return choices.some(
    ({ reason }) => reason === "decider_unapproved" || reason === "no_decider",
  );
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

/** The Answer Deadlines the server accepts, in seconds: a minute to 30 days. */
export const answerDeadlineBounds = {
  min: 60,
  max: 30 * 24 * 60 * 60,
} as const;

export const deadlineUnits = ["minutes", "hours", "days"] as const;

export type DeadlineUnit = (typeof deadlineUnits)[number];

const unitSeconds: Record<DeadlineUnit, number> = {
  minutes: 60,
  hours: 60 * 60,
  days: 24 * 60 * 60,
};

/** What the Answer Deadline field holds while the person edits it. */
export type DeadlineDraft =
  | { none: true }
  | { none: false; amount: string; unit: DeadlineUnit };

/**
 * The draft that shows a stored Answer Deadline: the largest unit that holds it
 * exactly. A value the API stored in seconds, such as 90, shows as minutes with
 * a fraction rather than rounding.
 */
export function deadlineDraft(seconds: number | null): DeadlineDraft {
  if (seconds === null) return { none: true };
  const unit =
    [...deadlineUnits]
      .reverse()
      .find((candidate) => seconds % unitSeconds[candidate] === 0) ?? "minutes";
  return { none: false, amount: String(seconds / unitSeconds[unit]), unit };
}

export type DeadlineProblem = "amount_required" | "too_short" | "too_long";

export type DeadlineResult =
  | { ok: true; seconds: number | null }
  | { ok: false; problem: DeadlineProblem };

/** The seconds a draft asks for, or why the server would refuse it. */
export function deadlineSeconds(draft: DeadlineDraft): DeadlineResult {
  if (draft.none) return { ok: true, seconds: null };
  const text = draft.amount.trim();
  const amount = Number(text);
  if (text === "" || !Number.isFinite(amount) || amount <= 0)
    return { ok: false, problem: "amount_required" };
  const seconds = Math.round(amount * unitSeconds[draft.unit]);
  if (seconds < answerDeadlineBounds.min)
    return { ok: false, problem: "too_short" };
  if (seconds > answerDeadlineBounds.max)
    return { ok: false, problem: "too_long" };
  return { ok: true, seconds };
}

/**
 * The body that sets, or with `null` clears, the Answer Deadline. The server
 * replaces the whole row, so the default mode is echoed back. Acceptance of
 * Bypass is recorded once, when the person first chooses it, never here.
 */
export function deadlineUpdate(
  preferences: { revision: number; default_mode: ApprovalMode },
  seconds: number | null,
): RequestApprovalPreferencesUpdate {
  return {
    expected_revision: preferences.revision,
    default_mode: preferences.default_mode,
    answer_deadline_seconds: seconds,
    accept_bypass: false,
  };
}

/**
 * When the Run waiting for `request` continues without an answer, or null: the
 * request is answered already, or its person set no Answer Deadline. The state
 * fills the field in only for a model's question.
 */
export function continuesAt(request: {
  response: unknown;
  answer_by?: string | null;
}): string | null {
  return request.response == null ? (request.answer_by ?? null) : null;
}

/**
 * Whether `request`, a human request or the payload of its answered event, was
 * closed by the system because its person's Answer Deadline passed. An approval
 * that expired is answered by the system too, so the response is read as well;
 * a person's own answer is never taken for this one.
 */
export function answeredAtDeadline(request: unknown): boolean {
  if (!request || typeof request !== "object") return false;
  const { response, answered_by } = request as Record<string, unknown>;
  if (answered_by !== "system") return false;
  if (!response || typeof response !== "object") return false;
  const { answered, reason } = response as Record<string, unknown>;
  return answered === false && reason === "answer_deadline";
}

/** The moment shown for a deadline, which can be days away. */
export function deadlineTime(at: string, locale: string): string {
  return new Date(at).toLocaleString(locale, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
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
export type GivenBy =
  | { by: "rule"; rule: string }
  | { by: "bypass" }
  | { by: "auto" };

export function givenBy(value: unknown): GivenBy | null {
  if (!value || typeof value !== "object") return null;
  const { rule, mode } = value as Record<string, unknown>;
  if (mode === "bypass") return { by: "bypass" };
  if (mode === "auto") return { by: "auto" };
  if (typeof rule === "string" && rule !== "") return { by: "rule", rule };
  return null;
}

/** Why the approval Decider blocked a call, by the name the model is told. */
export const blockReasons = [
  "data_exfiltration",
  "destructive",
  "beyond_request",
  "injected",
  "boundary",
  "unavailable",
] as const;

export type BlockReason = (typeof blockReasons)[number];

/** What refused an approval before anyone was asked. */
export type RefusedBy =
  | { by: "rule"; rule: string }
  | { by: "dont_ask" }
  | { by: "auto"; reason: BlockReason | null };

/**
 * `refused_by` of an approval card. A refusal by the Tenant's policy has
 * another shape and needs no reading: the card already says nobody may
 * approve it. A block reason this dashboard does not know is kept as `null`,
 * so the card still says that Auto blocked the call.
 */
export function refusedBy(value: unknown): RefusedBy | null {
  if (!value || typeof value !== "object") return null;
  const { rule, mode, reason } = value as Record<string, unknown>;
  if (mode === "dont_ask") return { by: "dont_ask" };
  if (mode === "auto")
    return {
      by: "auto",
      reason: blockReasons.find((known) => known === reason) ?? null,
    };
  if (typeof rule === "string" && rule !== "") return { by: "rule", rule };
  return null;
}

export type ThreadModeOption = {
  /** `default` clears the Thread's own mode. */
  value: "default" | ApprovalMode;
  mode: ApprovalMode;
  /** Why the option cannot be chosen; none when it can. */
  unavailable: ApprovalModeReason | null;
};

export type ThreadModeView = {
  mode: ApprovalMode | null;
  default_mode: ApprovalMode;
  modes: readonly ApprovalModeChoice[];
};

/**
 * The options of a Thread's mode switcher and the one in effect. The server
 * refuses a mode it reports a reason for, Bypass included until the person
 * accepted its risk as their default, so each reason makes its option
 * unavailable. The option already in effect is always listed, even when the
 * ceiling has since lowered.
 */
export function threadModeOptions(view: ThreadModeView): {
  selected: "default" | ApprovalMode;
  options: ThreadModeOption[];
} {
  return {
    selected: view.mode ?? "default",
    options: [
      { value: "default", mode: view.default_mode, unavailable: null },
      ...approvalModes.map((mode) => ({
        value: mode,
        mode,
        unavailable: modeReason(view.modes, mode),
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
