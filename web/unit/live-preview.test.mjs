import test from "node:test";
import assert from "node:assert/strict";
import {
  formatDuration,
  livePreviewIsActive,
  livePreviewIsViewable,
  livePreviewState,
  livePreviewStateKey,
  livePreviewStopReasonKey,
  livePreviewTicketErrorKey,
  mayStopLivePreview,
  remainingSeconds,
} from "../src/capabilities/live-preview-model.ts";

test("Live preview states map to labels and activity", () => {
  assert.equal(livePreviewState("running"), "running");
  assert.equal(livePreviewState("stale"), "stale");
  assert.equal(livePreviewState("stopped"), "stopped");
  assert.equal(livePreviewState("something-new"), "unknown");
  assert.equal(livePreviewState(null), "unknown");
  assert.equal(livePreviewStateKey("stale"), "livePreviewState_stale");
  assert.equal(livePreviewIsActive("running"), true);
  assert.equal(livePreviewIsActive("stale"), true);
  assert.equal(livePreviewIsActive("starting"), true);
  assert.equal(livePreviewIsActive("stopped"), false);
  assert.equal(livePreviewIsActive("unknown"), false);
  assert.equal(livePreviewIsViewable("running"), true);
  assert.equal(livePreviewIsViewable("stale"), true);
  assert.equal(livePreviewIsViewable("starting"), false);
  assert.equal(livePreviewIsViewable("stopped"), false);
});

test("every stop reason gets a plain-language key", () => {
  const expected = {
    idle: "livePreviewStop_idle",
    absolute: "livePreviewStop_absolute",
    run_cancelled: "livePreviewStop_run_cancelled",
    run_authority_revoked: "livePreviewStop_run_authority_revoked",
    tenant_deleted: "livePreviewStop_tenant_deleted",
    replaced: "livePreviewStop_replaced",
    area_cleaned_up: "livePreviewStop_area_cleaned_up",
    start_failed: "livePreviewStop_start_failed",
    stopped: "livePreviewStop_stopped",
    unexpected: "livePreviewStop_unknown",
  };
  for (const [reason, key] of Object.entries(expected))
    assert.equal(livePreviewStopReasonKey(reason), key);
  assert.equal(livePreviewStopReasonKey(null), "livePreviewStop_unknown");
});

test("absolute remaining time is rounded up and never negative", () => {
  const expires = "2026-10-26T02:00:00Z";
  assert.equal(
    remainingSeconds(expires, Date.parse("2026-10-26T01:59:59.200Z")),
    1,
  );
  assert.equal(
    remainingSeconds(expires, Date.parse("2026-10-26T02:00:00Z")),
    0,
  );
  assert.equal(
    remainingSeconds(expires, Date.parse("2026-10-26T03:00:00Z")),
    0,
  );
  assert.equal(remainingSeconds("not a date", Date.now()), 0);
  assert.equal(formatDuration(0), "0:00:00");
  assert.equal(formatDuration(59), "0:00:59");
  assert.equal(formatDuration(3661), "1:01:01");
  assert.equal(formatDuration(90000), "25:00:00");
});

test("Stop is offered for an active preview and never for a stopped one", () => {
  assert.equal(mayStopLivePreview("deployment_administrator", "running"), true);
  assert.equal(mayStopLivePreview("deployment_administrator", "stale"), true);
  assert.equal(
    mayStopLivePreview("deployment_administrator", "stopped"),
    false,
  );
  assert.equal(mayStopLivePreview("subject", "running"), true);
  assert.equal(mayStopLivePreview("subject", "stale"), true);
  assert.equal(mayStopLivePreview("subject", "stopped"), false);
});

test("Live preview ticket failures map to actionable states", () => {
  assert.equal(
    livePreviewTicketErrorKey({ status: 403 }),
    "filePreviewNoAccessNow",
  );
  assert.equal(
    livePreviewTicketErrorKey({ status: 404 }),
    "livePreviewTicketGone",
  );
  assert.equal(
    livePreviewTicketErrorKey({ status: 503 }),
    "filePreviewPreviewUnavailable",
  );
  assert.equal(
    livePreviewTicketErrorKey(new Error("offline")),
    "filePreviewTicketError",
  );
});
