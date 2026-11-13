import { test } from "node:test";
import assert from "node:assert/strict";
import {
  definitionOf,
  emptyForm,
  formOf,
  formProblems,
  haltText,
  patchOf,
  scheduleActions,
  quietCompletion,
  scheduleNoticeEntries,
  tickLabel,
  triggerSummary,
} from "../src/schedules-model.ts";

function filled() {
  return {
    ...emptyForm("Asia/Tokyo"),
    title: " Daily report ",
    request: "Summarize yesterday.",
    agentId: "reporter",
    agentVersion: "1.0.0",
  };
}

test("a filled form becomes the registration body, defaulting to a new Thread and the person's mode", () => {
  const body = definitionOf(filled());
  assert.deepEqual(body, {
    title: "Daily report",
    request: "Summarize yesterday.",
    agent: { id: "reporter", version: "1.0.0" },
    trigger: {
      kind: "cron",
      expression: "0 9 * * 1-5",
      time_zone: "Asia/Tokyo",
    },
    thread: { kind: "new" },
    max_lateness_seconds: 3600,
  });
});

test("a fixed Thread, an interval and a chosen mode reach the body", () => {
  const body = definitionOf({
    ...filled(),
    timing: "interval",
    everyMinutes: "120",
    thread: "11111111-1111-4111-8111-111111111111",
    approvalMode: "dont_ask",
  });
  assert.deepEqual(body.trigger, { kind: "interval", every_seconds: 7200 });
  assert.deepEqual(body.thread, {
    kind: "fixed",
    thread_id: "11111111-1111-4111-8111-111111111111",
  });
  assert.equal(body.approval_mode, "dont_ask");
});

test("an edit names the person's default mode explicitly", () => {
  assert.equal(patchOf(filled()).approval_mode, "default");
  assert.equal(
    patchOf({ ...filled(), approvalMode: "auto" }).approval_mode,
    "auto",
  );
});

test("the form refuses what the browser can tell is wrong", () => {
  assert.deepEqual(formProblems(filled(), 900), []);
  assert.deepEqual(
    formProblems({ ...emptyForm("UTC"), expression: "0 9 * *" }, 900),
    ["title", "request", "agent", "expression"],
  );
  assert.deepEqual(
    formProblems({ ...filled(), timing: "interval", everyMinutes: "10" }, 900),
    ["interval"],
  );
  assert.deepEqual(
    formProblems({ ...filled(), maxLatenessMinutes: "0" }, 900),
    ["lateness"],
  );
});

test("a stored Schedule round-trips through the form", () => {
  const schedule = {
    title: "Weekly",
    request: "Check dependencies.",
    agent: { id: "reporter", version: "1.2.0" },
    trigger: { kind: "interval", every_seconds: 86400 },
    thread: { kind: "new" },
    approval_mode: null,
    max_lateness_seconds: 600,
  };
  const form = formOf(schedule);
  assert.equal(form.everyMinutes, "1440");
  assert.equal(form.approvalMode, "default");
  assert.deepEqual(definitionOf(form).trigger, schedule.trigger);
  assert.equal(triggerSummary(schedule.trigger, false), "every 1 d");
  assert.equal(triggerSummary(schedule.trigger, true), "1日ごと");
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

test("stopped Schedules become pending bell entries in their own scope", () => {
  const { scope, entries } = scheduleNoticeEntries(
    "home",
    {
      stopped: [
        {
          id: "s1",
          workspace_id: "w1",
          title: "Weekly",
          registrant: "alice",
          halt: "registrant_left",
          halted_at: "2026-11-13T00:00:00Z",
          successor: null,
        },
      ],
      scheduled_tasks: [],
    },
    () => "Ops",
    false,
  );
  assert.equal(scope, JSON.stringify(["home", "schedules"]));
  assert.equal(entries.length, 1);
  assert.equal(entries[0].kind, "schedule_stopped");
  assert.equal(entries[0].target.kind, "schedule");
  assert.equal(entries[0].workspaceTitle, "Ops");
  assert.match(entries[0].detail, /Waiting for a successor/);
});

test("a notices answer without a stopped list yields no entries instead of throwing", () => {
  for (const answer of [[], {}, null]) {
    const { entries } = scheduleNoticeEntries("home", answer, () => "", false);
    assert.deepEqual(entries, []);
  }
});

test("only a scheduled Task's completion is quiet", () => {
  const scheduled = new Set(["t1"]);
  assert.equal(quietCompletion("completed", "t1", scheduled), true);
  assert.equal(quietCompletion("failed", "t1", scheduled), false);
  assert.equal(quietCompletion("completed", "t2", scheduled), false);
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
  // The Registrant's own active Schedule.
  assert.deepEqual(
    scheduleActions(row({ mine: true, may_edit: true, may_manage: true })),
    { edit: true, pause: true, resume: null, remove: true },
  );
  // Someone else's, seen through the Workspace: nothing to change.
  assert.deepEqual(scheduleActions(row({})), {
    edit: false,
    pause: false,
    resume: null,
    remove: false,
  });
  // An administrator pauses and deletes but never resumes it.
  assert.deepEqual(
    scheduleActions(row({ status: "paused", may_manage: true })),
    { edit: false, pause: false, resume: null, remove: true },
  );
  // Waiting for a successor: only the named successor takes it over.
  assert.equal(
    scheduleActions(row({ status: "stopped", may_resume: true })).resume,
    "takeOver",
  );
  assert.equal(
    scheduleActions(row({ status: "stopped", mine: true, may_resume: false }))
      .resume,
    null,
  );
});

test("a stop names the server's reason when it gave one", () => {
  assert.equal(
    haltText(
      "interval_too_short",
      "the Tenant's minimum interval is now 120 minutes",
      false,
    ),
    "Stopped: shorter than the Tenant's minimum interval. Change it and resume (the Tenant's minimum interval is now 120 minutes)",
  );
  assert.equal(haltText("paused", null, false), "Paused");
});
