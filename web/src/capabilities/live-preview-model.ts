/** Pure Live preview state, reason and time mapping for the dashboard. */

export type LivePreviewState =
  | "starting"
  | "running"
  | "stale"
  | "stopped"
  | "unknown";

export function livePreviewState(
  value: string | null | undefined,
): LivePreviewState {
  switch (value) {
    case "starting":
    case "running":
    case "stale":
    case "stopped":
      return value;
    default:
      return "unknown";
  }
}

export function livePreviewStateKey(state: LivePreviewState): string {
  return `livePreviewState_${state}`;
}

/** A Live preview can be viewed while it still serves or is about to serve. */
export function livePreviewIsActive(state: LivePreviewState): boolean {
  return state === "starting" || state === "running" || state === "stale";
}

/** Only a serving preview can be shown; "starting" has no port yet. */
export function livePreviewIsViewable(state: LivePreviewState): boolean {
  return state === "running" || state === "stale";
}

/** Plain-language reason key for a stopped Live preview. */
export function livePreviewStopReasonKey(
  reason: string | null | undefined,
): string {
  switch (reason) {
    case "idle":
      return "livePreviewStop_idle";
    case "absolute":
      return "livePreviewStop_absolute";
    case "run_cancelled":
      return "livePreviewStop_run_cancelled";
    case "run_authority_revoked":
      return "livePreviewStop_run_authority_revoked";
    case "tenant_deleted":
      return "livePreviewStop_tenant_deleted";
    case "replaced":
      return "livePreviewStop_replaced";
    case "area_cleaned_up":
      return "livePreviewStop_area_cleaned_up";
    case "start_failed":
      return "livePreviewStop_start_failed";
    case "stopped":
      return "livePreviewStop_stopped";
    default:
      return "livePreviewStop_unknown";
  }
}

/** Seconds until the absolute limit, rounded up and never negative. */
export function remainingSeconds(expiresAt: string, nowMs: number): number {
  const expires = Date.parse(expiresAt);
  if (!Number.isFinite(expires)) return 0;
  return Math.max(0, Math.ceil((expires - nowMs) / 1000));
}

export function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  return `${hours}:${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}`;
}

/**
 * The current Live preview API has no per-viewer may_stop field. The
 * Deployment Administrator is the only viewer the dashboard can prove may
 * cancel the Run, so Stop is limited to that case until the server exposes
 * the requester's own permission.
 */
export function mayStopLivePreview(
  accessKind: "subject" | "deployment_administrator",
  state: LivePreviewState,
): boolean {
  return (
    accessKind === "deployment_administrator" && livePreviewIsActive(state)
  );
}

/** Ticket errors for a Live preview. */
export function livePreviewTicketErrorKey(error: unknown): string {
  const status =
    typeof error === "object" &&
    error !== null &&
    "status" in error &&
    typeof (error as { status?: unknown }).status === "number"
      ? (error as { status: number }).status
      : undefined;
  if (status === 403) return "filePreviewNoAccessNow";
  if (status === 404) return "livePreviewTicketGone";
  if (status === 409 || status === 503) return "filePreviewPreviewUnavailable";
  return "filePreviewTicketError";
}
