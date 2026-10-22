import { test } from "node:test";
import assert from "node:assert/strict";
import {
  answerDeadlineBounds,
  answeredAtDeadline,
  approvalKinds,
  approvalModes,
  blockReasons,
  bypassNeedsConfirmation,
  continuesAt,
  deadlineDraft,
  deadlineSeconds,
  deadlineSummary,
  deadlineTime,
  deadlineUpdate,
  decisionScope,
  givenBy,
  modeAllowed,
  modeReason,
  needsApprovalDecider,
  preferencesUpdate,
  refusedBy,
  ruleInput,
  ruleTargetText,
  targetExamples,
  threadModeOptions,
  threadModeUpdate,
} from "../src/approval-settings-model.ts";
import {
  approvalCopy,
  givenByText,
  refusedByText,
} from "../src/approval-settings-copy.ts";

const choices = (allowed, reasons = {}) =>
  approvalModes.map((mode) => ({
    mode,
    allowed: allowed.includes(mode),
    reason: reasons[mode] ?? (allowed.includes(mode) ? null : "policy"),
  }));

test("a mode is offered only when the Tenant's policy reports it allowed", () => {
  const modes = choices(["manual", "dont_ask"]);
  assert.equal(modeAllowed(modes, "manual"), true);
  assert.equal(modeAllowed(modes, "bypass"), false);
  assert.equal(modeAllowed([], "manual"), false);
});

test("the first choice of Bypass asks for confirmation, later ones do not", () => {
  const first = { bypass_accepted_at: null };
  const accepted = { bypass_accepted_at: "2026-10-19T00:00:00Z" };
  assert.equal(bypassNeedsConfirmation(first, "bypass"), true);
  assert.equal(bypassNeedsConfirmation(accepted, "bypass"), false);
  for (const mode of ["manual", "auto", "dont_ask"])
    assert.equal(bypassNeedsConfirmation(first, mode), false);
});

test("a default mode is saved with the revision and keeps the Answer Deadline", () => {
  const stored = { revision: 4, answer_deadline_seconds: 600 };
  assert.deepEqual(preferencesUpdate(stored, "dont_ask", false), {
    expected_revision: 4,
    default_mode: "dont_ask",
    answer_deadline_seconds: 600,
    accept_bypass: false,
  });
  assert.equal(
    preferencesUpdate(
      { revision: 0, answer_deadline_seconds: null },
      "manual",
      false,
    ).answer_deadline_seconds,
    null,
  );
});

test("a member's Answer Deadline reads in the form's largest exact unit", () => {
  const units = { minutes: "Minutes", hours: "Hours", days: "Days" };
  assert.equal(deadlineSummary(null, units, "None"), "None");
  assert.equal(deadlineSummary(300, units, "None"), "5 Minutes");
  assert.equal(deadlineSummary(7200, units, "None"), "2 Hours");
  assert.equal(deadlineSummary(90, units, "None"), "1.5 Minutes");
});

test("Bypass acceptance is sent only for Bypass, and only once confirmed", () => {
  const stored = { revision: 1, answer_deadline_seconds: null };
  assert.equal(preferencesUpdate(stored, "bypass", true).accept_bypass, true);
  assert.equal(preferencesUpdate(stored, "bypass", false).accept_bypass, false);
  assert.equal(preferencesUpdate(stored, "manual", true).accept_bypass, false);
});

test("a rule names every target of its kind or exactly one", () => {
  assert.deepEqual(
    ruleInput({ kind: "outbound", every: true, target: "", effect: "deny" }),
    {
      ok: true,
      input: { kind: "outbound", effect: "deny", target: "every" },
    },
  );
  assert.deepEqual(
    ruleInput({
      kind: "tool_call",
      every: false,
      target: "  tool:crm#invoke ",
      effect: "allow",
    }),
    {
      ok: true,
      input: {
        kind: "tool_call",
        effect: "allow",
        target: { exact: "tool:crm#invoke" },
      },
    },
  );
});

test("an exact rule without a target is not sent", () => {
  for (const target of ["", "   "])
    assert.deepEqual(
      ruleInput({ kind: "outbound", every: false, target, effect: "ask" }),
      { ok: false, problem: "target_required" },
    );
});

test("every kind has an example target", () => {
  for (const kind of approvalKinds) assert.ok(targetExamples[kind], kind);
});

test("a rule's target reads back as text or as every target", () => {
  assert.equal(ruleTargetText("every"), null);
  assert.equal(ruleTargetText({ exact: "example.com" }), "example.com");
});

test("an approval given in advance names its rule, Bypass or Auto", () => {
  assert.deepEqual(givenBy({ rule: "3f2c" }), { by: "rule", rule: "3f2c" });
  assert.deepEqual(givenBy({ mode: "bypass" }), { by: "bypass" });
  assert.deepEqual(givenBy({ mode: "auto", decision: "4d1e" }), {
    by: "auto",
  });
  for (const unknown of [
    null,
    undefined,
    "bypass",
    {},
    { rule: "" },
    { mode: "manual" },
  ])
    assert.equal(givenBy(unknown), null);
});

test("a refusal in advance names its rule, Don't ask, or Auto's block reason", () => {
  assert.deepEqual(refusedBy({ rule: "9a0b" }), { by: "rule", rule: "9a0b" });
  assert.deepEqual(refusedBy({ mode: "dont_ask" }), { by: "dont_ask" });
  for (const reason of blockReasons)
    assert.deepEqual(refusedBy({ mode: "auto", reason }), {
      by: "auto",
      reason,
    });
});

test("an unknown block reason still reads as a block by Auto", () => {
  assert.deepEqual(refusedBy({ mode: "auto", reason: "vibes" }), {
    by: "auto",
    reason: null,
  });
  assert.deepEqual(refusedBy({ mode: "auto" }), { by: "auto", reason: null });
});

test("a refusal this dashboard has nothing to say about is not read", () => {
  for (const unknown of [
    null,
    undefined,
    "dont_ask",
    {},
    { rule: "" },
    { policy: "tenant" },
    { mode: "bypass" },
  ])
    assert.equal(refusedBy(unknown), null);
});

test("a card says who gave an approval, to its owner and to others", () => {
  const words = approvalCopy["en-US"];
  assert.equal(
    givenByText(words, { by: "rule", rule: "r" }, true, "alice"),
    "Allowed by your rule",
  );
  assert.equal(
    givenByText(words, { by: "bypass" }, true, "alice"),
    "Allowed by Bypass",
  );
  assert.equal(
    givenByText(words, { by: "rule", rule: "r" }, false, "alice"),
    "Allowed by alice's rule",
  );
  for (const mine of [true, false])
    assert.equal(
      givenByText(words, { by: "auto" }, mine, "alice"),
      "Allowed by Auto",
    );
});

test("a card says what refused an approval, with Auto's reason in words", () => {
  const words = approvalCopy["en-US"];
  const text = (refused, mine = true) =>
    refusedByText(words, refused, mine, "alice");
  assert.equal(text({ by: "rule", rule: "r" }), "Refused by your rule");
  assert.equal(text({ by: "dont_ask" }), "Refused: Don't ask");
  assert.equal(
    text({ by: "rule", rule: "r" }, false),
    "Refused by alice's rule",
  );
  assert.equal(text({ by: "dont_ask" }, false), "Refused: alice's Don't ask");
  const blocked = {
    data_exfiltration: "data sent beyond your request",
    destructive: "destroys or irreversibly changes things",
    beyond_request: "goes beyond your request",
    injected: "driven by content the Run read",
    boundary: "contradicts a boundary you stated",
    unavailable: "the Decider was unavailable",
  };
  assert.deepEqual(Object.keys(blocked), [...blockReasons]);
  for (const [reason, words_] of Object.entries(blocked))
    for (const mine of [true, false])
      assert.equal(
        text({ by: "auto", reason }, mine),
        `Blocked by Auto: ${words_}`,
      );
  assert.equal(text({ by: "auto", reason: null }), "Blocked by Auto");
});

test("every copy names every block reason and mode reason in both languages", () => {
  for (const words of Object.values(approvalCopy)) {
    for (const reason of blockReasons)
      assert.ok(words.blockReasons[reason], reason);
    for (const reason of [
      "policy",
      "not_accepted",
      "decider_unapproved",
      "no_decider",
    ]) {
      assert.ok(words.modeReason[reason]("aidash.approval-decider@1.0.0"));
      assert.ok(words.threadUnavailable[reason], reason);
    }
  }
});

test("a mode's reason names the Decider only when it is known", () => {
  const words = approvalCopy["en-US"].modeReason;
  assert.match(
    words.decider_unapproved("aidash.approval-decider@1.0.0"),
    /\(aidash\.approval-decider@1\.0\.0\) has no Catalog Approval/,
  );
  assert.doesNotMatch(words.decider_unapproved(""), /\(/);
  assert.match(words.not_accepted(""), /first choose it as your default/);
});

const view = (mode, allowed = ["manual", "auto", "dont_ask", "bypass"]) => ({
  mode,
  default_mode: "manual",
  modes: choices(allowed),
});

test("a Thread without its own mode shows the default and offers every allowed mode", () => {
  const { selected, options } = threadModeOptions(view(null));
  assert.equal(selected, "default");
  assert.deepEqual(
    options.map((option) => [option.value, option.mode, option.unavailable]),
    [
      ["default", "manual", null],
      ["manual", "manual", null],
      ["auto", "auto", null],
      ["dont_ask", "dont_ask", null],
      ["bypass", "bypass", null],
    ],
  );
});

test("a Thread's own mode is the selected option", () => {
  assert.equal(threadModeOptions(view("dont_ask")).selected, "dont_ask");
});

test("modes the policy withholds are unavailable with the policy as the reason", () => {
  const { options } = threadModeOptions(view(null, ["manual", "dont_ask"]));
  const reason = (value) => options.find((o) => o.value === value).unavailable;
  assert.equal(reason("auto"), "policy");
  assert.equal(reason("bypass"), "policy");
  assert.equal(reason("dont_ask"), null);
});

test("Bypass stays unavailable in a Thread until the person accepted its risk", () => {
  const { options } = threadModeOptions({
    ...view(null),
    modes: choices(["manual", "auto", "dont_ask", "bypass"], {
      bypass: "not_accepted",
    }),
  });
  const bypass = options.find((o) => o.value === "bypass");
  assert.equal(bypass.unavailable, "not_accepted");
});

test("Auto is unavailable in a Thread for the Decider's reason, not the policy's", () => {
  for (const reason of ["decider_unapproved", "no_decider"]) {
    const { options } = threadModeOptions({
      ...view(null),
      modes: choices(["manual", "dont_ask", "bypass"], { auto: reason }),
    });
    assert.equal(options.find((o) => o.value === "auto").unavailable, reason);
  }
});

test("a mode's availability and reason come from the server's choices", () => {
  const modes = choices(["manual", "auto", "bypass"], {
    bypass: "not_accepted",
    dont_ask: "policy",
  });
  assert.equal(modeAllowed(modes, "bypass"), true);
  assert.equal(modeReason(modes, "bypass"), "not_accepted");
  assert.equal(modeReason(modes, "dont_ask"), "policy");
  assert.equal(modeReason(modes, "manual"), null);
  assert.equal(modeReason([], "manual"), null);
});

test("only a mode held back by the approval Decider asks for its name", () => {
  const without = choices(["manual", "dont_ask"]);
  assert.equal(needsApprovalDecider(without), false);
  for (const reason of ["decider_unapproved", "no_decider"])
    assert.equal(
      needsApprovalDecider(
        choices(["manual", "dont_ask", "bypass"], { auto: reason }),
      ),
      true,
    );
});

test("a Thread mode is set by name and cleared by the default", () => {
  assert.deepEqual(threadModeUpdate("dont_ask"), { mode: "dont_ask" });
  assert.deepEqual(threadModeUpdate("default"), { mode: null });
});

test("only Allow for this Run carries a scope", () => {
  const grant = { targets: ["tool:crm@1#invoke"], minutes: 15, now: 1_000_000 };
  assert.deepEqual(decisionScope("allow_run", grant), {
    targets: ["tool:crm@1#invoke"],
    expires_at: new Date(1_000_000 + 15 * 60000).toISOString(),
  });
  for (const choice of ["allow_once", "allow_always", "deny"])
    assert.deepEqual(decisionScope(choice, grant), {
      targets: null,
      expires_at: null,
    });
});

test("a stored Answer Deadline shows in the largest unit that holds it exactly", () => {
  assert.deepEqual(deadlineDraft(null), { none: true });
  assert.deepEqual(deadlineDraft(300), {
    none: false,
    amount: "5",
    unit: "minutes",
  });
  assert.deepEqual(deadlineDraft(7200), {
    none: false,
    amount: "2",
    unit: "hours",
  });
  assert.deepEqual(deadlineDraft(2 * 86400), {
    none: false,
    amount: "2",
    unit: "days",
  });
  assert.deepEqual(deadlineDraft(90), {
    none: false,
    amount: "1.5",
    unit: "minutes",
  });
});

test("an Answer Deadline round-trips through its draft without drifting", () => {
  for (const seconds of [60, 61, 90, 300, 3600, 5400, 86400, 30 * 86400])
    assert.deepEqual(deadlineSeconds(deadlineDraft(seconds)), {
      ok: true,
      seconds,
    });
});

test("no Answer Deadline is none, and a duration is counted in its unit", () => {
  assert.deepEqual(deadlineSeconds({ none: true }), {
    ok: true,
    seconds: null,
  });
  const draft = (amount, unit) => ({ none: false, amount, unit });
  assert.deepEqual(deadlineSeconds(draft("5", "minutes")), {
    ok: true,
    seconds: 300,
  });
  assert.deepEqual(deadlineSeconds(draft(" 2 ", "hours")), {
    ok: true,
    seconds: 7200,
  });
  assert.deepEqual(deadlineSeconds(draft("1.5", "days")), {
    ok: true,
    seconds: 129600,
  });
});

test("an Answer Deadline outside a minute to 30 days is refused before it is sent", () => {
  const draft = (amount, unit) => ({ none: false, amount, unit });
  for (const amount of ["", "  ", "0", "-5", "soon", "Infinity"])
    assert.deepEqual(deadlineSeconds(draft(amount, "minutes")), {
      ok: false,
      problem: "amount_required",
    });
  assert.equal(deadlineSeconds(draft("0.5", "minutes")).problem, "too_short");
  assert.equal(deadlineSeconds(draft("1", "minutes")).ok, true);
  assert.equal(deadlineSeconds(draft("720", "hours")).ok, true);
  assert.equal(deadlineSeconds(draft("721", "hours")).problem, "too_long");
  assert.equal(deadlineSeconds(draft("30", "days")).ok, true);
  assert.equal(deadlineSeconds(draft("30.01", "days")).problem, "too_long");
  assert.equal(
    answerDeadlineBounds.max,
    deadlineSeconds(draft("30", "days")).seconds,
  );
});

test("saving the Answer Deadline keeps the default mode and never records Bypass", () => {
  const stored = { revision: 7, default_mode: "bypass" };
  assert.deepEqual(deadlineUpdate(stored, 900), {
    expected_revision: 7,
    default_mode: "bypass",
    answer_deadline_seconds: 900,
    accept_bypass: false,
  });
  assert.equal(deadlineUpdate(stored, null).answer_deadline_seconds, null);
});

test("a waiting question shows when its Run continues, and an answered one shows nothing", () => {
  const at = "2026-10-21T10:30:00Z";
  assert.equal(continuesAt({ response: null, answer_by: at }), at);
  assert.equal(continuesAt({ response: null, answer_by: null }), null);
  assert.equal(continuesAt({ response: null }), null);
  assert.equal(
    continuesAt({ response: { approved: true }, answer_by: at }),
    null,
  );
});

test("only the system's answer at the Answer Deadline reads as no answer", () => {
  const missed = { answered: false, reason: "answer_deadline" };
  assert.equal(
    answeredAtDeadline({ response: missed, answered_by: "system" }),
    true,
  );
  assert.equal(
    answeredAtDeadline({ response: missed, answered_by: "alice" }),
    false,
  );
  assert.equal(
    answeredAtDeadline({
      response: { approved: false, expired: true },
      answered_by: "system",
    }),
    false,
  );
  assert.equal(
    answeredAtDeadline({ response: "tokyo", answered_by: "system" }),
    false,
  );
  for (const other of [null, undefined, "system", { response: null }])
    assert.equal(answeredAtDeadline(other), false);
});

test("a deadline shows with its date, since it can be days away", () => {
  const at = "2026-10-21T10:30:00Z";
  assert.match(deadlineTime(at, "en-US"), /Oct/);
  assert.match(deadlineTime(at, "ja-JP"), /10月/);
});

test("both locales explain the Answer Deadline and its limits", () => {
  const english = approvalCopy["en-US"].deadline;
  const japanese = approvalCopy["ja-JP"].deadline;
  assert.match(english.help, /continues on its own judgment/);
  assert.match(english.help, /never applies to Completion Reviews/);
  assert.match(japanese.help, /Completion Review/);
  assert.deepEqual(
    Object.keys(english.problems),
    Object.keys(japanese.problems),
  );
  assert.deepEqual(Object.keys(english.units), ["minutes", "hours", "days"]);
});
