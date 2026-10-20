import { test } from "node:test";
import assert from "node:assert/strict";
import {
  approvalKinds,
  approvalModes,
  bypassNeedsConfirmation,
  decisionScope,
  givenBy,
  modeAllowed,
  preferencesUpdate,
  ruleInput,
  ruleTargetText,
  targetExamples,
  threadModeOptions,
  threadModeUpdate,
} from "../src/approval-settings-model.ts";

const choices = (allowed) =>
  approvalModes.map((mode) => ({ mode, allowed: allowed.includes(mode) }));

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

test("an approval given in advance names its rule or Bypass", () => {
  assert.deepEqual(givenBy({ rule: "3f2c" }), { by: "rule", rule: "3f2c" });
  assert.deepEqual(givenBy({ mode: "bypass" }), { by: "bypass" });
  for (const unknown of [
    null,
    undefined,
    "bypass",
    {},
    { rule: "" },
    { mode: "auto" },
  ])
    assert.equal(givenBy(unknown), null);
});

const view = (mode, allowed = ["manual", "auto", "dont_ask", "bypass"]) => ({
  mode,
  default_mode: "manual",
  modes: choices(allowed),
});

test("a Thread without its own mode shows the default and offers every allowed mode", () => {
  const { selected, options } = threadModeOptions(view(null), true);
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
  assert.equal(threadModeOptions(view("dont_ask"), true).selected, "dont_ask");
});

test("modes the policy withholds are unavailable with the policy as the reason", () => {
  const { options } = threadModeOptions(
    view(null, ["manual", "dont_ask"]),
    true,
  );
  const reason = (value) => options.find((o) => o.value === value).unavailable;
  assert.equal(reason("auto"), "policy");
  assert.equal(reason("bypass"), "policy");
  assert.equal(reason("dont_ask"), null);
});

test("Bypass stays unavailable in a Thread until the person accepted its risk", () => {
  const { options } = threadModeOptions(view(null), false);
  const bypass = options.find((o) => o.value === "bypass");
  assert.equal(bypass.unavailable, "bypass_unaccepted");
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
