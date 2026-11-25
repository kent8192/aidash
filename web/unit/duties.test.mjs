import { test } from "node:test";
import assert from "node:assert/strict";
import {
  definitionOf,
  emptyForm,
  formOf,
  formProblems,
  haltText,
  patchOf,
  dutyActions,
  quietCompletion,
  dutyNoticeEntries,
  eventLabel,
  tickLabel,
  triggerSummary,
} from "../src/duties-model.ts";

function filled() {
  return {
    ...emptyForm("Asia/Tokyo"),
    title: " Daily report ",
    mission: "Summarize yesterday.",
    procedure: "1. Read every merged pull request.",
    agentId: "reporter",
    agentVersion: "1.0.0",
  };
}

test("a filled form becomes the registration body, defaulting to the person's mode and three failures", () => {
  const body = definitionOf(filled());
  assert.deepEqual(body, {
    title: "Daily report",
    mission: "Summarize yesterday.",
    procedure: "1. Read every merged pull request.",
    agent: { id: "reporter", version: "1.0.0" },
    trigger: {
      kind: "cron",
      expression: "0 9 * * 1-5",
      time_zone: "Asia/Tokyo",
    },
    max_lateness_seconds: 3600,
    pause_after_failures: 3,
  });
});

test("an interval, a chosen mode and never pausing reach the body", () => {
  const body = definitionOf({
    ...filled(),
    timing: "interval",
    everyMinutes: "120",
    approvalMode: "dont_ask",
    pauseAfterFailures: "0",
  });
  assert.deepEqual(body.trigger, { kind: "interval", every_seconds: 7200 });
  assert.equal(body.approval_mode, "dont_ask");
  assert.equal(body.pause_after_failures, 0);
});

test("a Duty without a time trigger sends none and an edit removes it", () => {
  const form = { ...filled(), timing: "none" };
  assert.equal("trigger" in definitionOf(form), false);
  assert.deepEqual(patchOf(form).trigger, { kind: "none" });
});

test("an edit names the person's default mode explicitly", () => {
  assert.equal(patchOf(filled()).approval_mode, "default");
  assert.equal(
    patchOf({ ...filled(), approvalMode: "auto" }).approval_mode,
    "auto",
  );
});

test("the form refuses what the browser can tell is wrong", () => {
  assert.deepEqual(formProblems(filled()), []);
  assert.deepEqual(
    formProblems({ ...emptyForm("UTC"), expression: "0 9 * *" }),
    ["title", "mission", "agent", "expression"],
  );
  // Any interval of a minute or more is fine: there is no minimum (#1012).
  assert.deepEqual(
    formProblems({ ...filled(), timing: "interval", everyMinutes: "1" }),
    [],
  );
  assert.deepEqual(
    formProblems({ ...filled(), timing: "interval", everyMinutes: "0" }),
    ["interval"],
  );
  assert.deepEqual(formProblems({ ...filled(), maxLatenessMinutes: "0" }), [
    "lateness",
  ]);
  assert.deepEqual(formProblems({ ...filled(), pauseAfterFailures: "-1" }), [
    "failures",
  ]);
});

test("a stored Duty round-trips through the form", () => {
  const duty = {
    title: "Weekly",
    mission: "Check dependencies.",
    procedure: "",
    agent: { id: "reporter", version: "1.2.0" },
    trigger: { kind: "interval", every_seconds: 86400 },
    approval_mode: null,
    max_lateness_seconds: 600,
    pause_after_failures: 3,
  };
  const form = formOf(duty);
  assert.equal(form.everyMinutes, "1440");
  assert.equal(form.approvalMode, "default");
  assert.deepEqual(definitionOf(form).trigger, duty.trigger);
  assert.equal(triggerSummary(duty.trigger, false), "every 1 d");
  assert.equal(triggerSummary(duty.trigger, true), "1日ごと");
});

test("saving a Duty kept in seconds does not round its timing", () => {
  const duty = {
    title: "Probe",
    mission: "Check the probe.",
    procedure: "",
    agent: { id: "reporter", version: "1.2.0" },
    trigger: { kind: "interval", every_seconds: 930 },
    approval_mode: null,
    max_lateness_seconds: 90,
    pause_after_failures: 3,
  };
  const renamed = { ...formOf(duty), title: "Probe 2" };
  const patch = patchOf(renamed, duty);
  assert.deepEqual(patch.trigger, duty.trigger);
  assert.equal(patch.max_lateness_seconds, 90);
  assert.equal(patch.title, "Probe 2");
  const retimed = patchOf({ ...renamed, everyMinutes: "30" }, duty);
  assert.deepEqual(retimed.trigger, { kind: "interval", every_seconds: 1800 });
});

test("tick labels give the skip reason and who ran it now", () => {
  const tick = {
    status: "skipped",
    reason: "limit: free_usage_exhausted",
    started_by: null,
  };
  assert.equal(tickLabel(tick, false), "skipped (limit)");
  assert.equal(
    tickLabel({ status: "started", reason: null, started_by: "carol" }, false),
    "started · run now by carol",
  );
  assert.equal(
    tickLabel(
      {
        status: "failed",
        reason: "COMPACTION_DECIDER_UNAPPROVED: d@1",
        started_by: null,
      },
      false,
    ),
    "failed (COMPACTION_DECIDER_UNAPPROVED: d@1)",
  );
});

test("stopped Duties become pending bell entries in their own scope", () => {
  const { scope, entries } = dutyNoticeEntries(
    "home",
    {
      stopped: [
        {
          id: "s1",
          thread_id: "w1",
          title: "Weekly",
          requester: "alice",
          halt: "requester_left",
          halted_at: "2026-11-13T00:00:00Z",
          successor: null,
        },
      ],
      scheduled_tasks: [],
    },
    () => "Ops",
    false,
  );
  assert.equal(scope, JSON.stringify(["home", "duties"]));
  assert.equal(entries.length, 1);
  assert.equal(entries[0].kind, "duty_stopped");
  assert.equal(entries[0].target.kind, "duty");
  assert.equal(entries[0].threadTitle, "Ops");
  assert.match(entries[0].detail, /Waiting for a successor/);
});

test("a notices answer without a stopped list yields no entries instead of throwing", () => {
  for (const answer of [[], {}, null]) {
    const { entries } = dutyNoticeEntries("home", answer, () => "", false);
    assert.deepEqual(entries, []);
  }
});

test("only a scheduled Task's completion is quiet", () => {
  assert.equal(quietCompletion("completed", "duty-id"), true);
  assert.equal(quietCompletion("failed", "duty-id"), false);
  assert.equal(quietCompletion("completed", null), false);
  assert.equal(quietCompletion("completed", undefined), false);
});

function row(overrides) {
  return {
    mine: false,
    status: "active",
    may_edit: false,
    may_manage: false,
    may_resume: false,
    ...overrides,
  };
}

test("a row offers only what the viewer may do", () => {
  // The Requester's own active Duty.
  assert.deepEqual(
    dutyActions(row({ mine: true, may_edit: true, may_manage: true })),
    { edit: true, pause: true, resume: null, remove: true },
  );
  // Someone else's, seen through the Thread: nothing to change.
  assert.deepEqual(dutyActions(row({})), {
    edit: false,
    pause: false,
    resume: null,
    remove: false,
  });
  // An administrator pauses and deletes but never resumes it.
  assert.deepEqual(dutyActions(row({ status: "paused", may_manage: true })), {
    edit: false,
    pause: false,
    resume: null,
    remove: true,
  });
  // Waiting for a successor: only the named successor takes it over.
  assert.equal(
    dutyActions(row({ status: "stopped", may_resume: true })).resume,
    "takeOver",
  );
  assert.equal(
    dutyActions(row({ status: "stopped", mine: true, may_resume: false }))
      .resume,
    null,
  );
});

test("a stop names the server's reason when it gave one", () => {
  assert.equal(
    haltText(
      "agent_version_unusable",
      "Agent reporter@1.0.0 can no longer be started",
      false,
    ),
    "Stopped: its Agent version can no longer be started (Agent reporter@1.0.0 can no longer be started)",
  );
  assert.equal(haltText("paused", null, false), "Paused");
});

test("a Router and a Watcher reach the registration body and an edit removes them", () => {
  const form = {
    ...filled(),
    routerOn: true,
    routerLanguage: "shell",
    routerSource: 'echo \'{"action":"drop"}\'',
    watcherOn: true,
    watcherLanguage: "python",
    watcherSource: "print('[]')",
    watcherEveryMinutes: "10",
  };
  const body = definitionOf(form);
  assert.deepEqual(body.router, {
    language: "shell",
    source: 'echo \'{"action":"drop"}\'',
  });
  assert.deepEqual(body.watcher, {
    script: { language: "python", source: "print('[]')" },
    every_seconds: 600,
  });
  const cleared = patchOf({ ...form, routerOn: false, watcherOn: false });
  assert.deepEqual(cleared.router, { kind: "none" });
  assert.deepEqual(cleared.watcher, { kind: "none" });
  assert.equal("router" in definitionOf(filled()), false);
});

test("a stored Watcher keeps its exact seconds and its scripts round-trip", () => {
  const duty = {
    title: "Watch CI",
    mission: "Fix failing CI.",
    procedure: "",
    agent: { id: "reporter", version: "1.2.0" },
    trigger: null,
    approval_mode: null,
    max_lateness_seconds: 3600,
    pause_after_failures: 3,
    router: { language: "python", source: "print('{}')" },
    watcher: {
      script: { language: "shell", source: "echo '[]'" },
      every_seconds: 90,
    },
  };
  const form = formOf(duty);
  assert.equal(form.routerOn, true);
  assert.equal(form.watcherSource, "echo '[]'");
  assert.deepEqual(patchOf(form, duty).watcher, duty.watcher);
  assert.deepEqual(patchOf(form, duty).router, duty.router);
});

test("the form refuses empty or oversized scripts and a Watcher interval past a year", () => {
  const base = { ...filled(), routerOn: true, watcherOn: true };
  assert.deepEqual(
    formProblems({ ...base, routerSource: " ", watcherSource: "x" }),
    ["router"],
  );
  assert.deepEqual(
    formProblems({
      ...base,
      routerSource: "x".repeat(64 * 1024 + 1),
      watcherSource: "x",
      watcherEveryMinutes: "0",
    }),
    ["router", "watcher_interval"],
  );
});

test("an event shows the Router's decision", () => {
  const task = {
    id: "e1",
    received_at: "2026-11-19T00:00:00Z",
    outcome: "started",
    reason: null,
    router: { action: "task", title: "Fix CI" },
  };
  assert.equal(
    eventLabel(task, false),
    "started a Task · Router: started a Task (Fix CI)",
  );
  assert.equal(
    eventLabel(
      {
        ...task,
        outcome: "failed",
        router: null,
        reason: "Router: the Router printed nothing",
      },
      false,
    ),
    "failed (Router: the Router printed nothing)",
  );
  assert.equal(
    eventLabel(
      { ...task, outcome: "dropped", router: { action: "drop" } },
      true,
    ),
    "破棄 · ルーター: 破棄",
  );
});
