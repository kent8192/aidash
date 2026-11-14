/** Schedules (#557, ADR 0134): what the API returns and the pure rules the
 *  Schedule screens and the notification bell share. */
import type { NoticeEntry } from "./notifications/model";

export type ApprovalMode = "manual" | "auto" | "dont_ask" | "bypass";
export type Trigger =
  | { kind: "cron"; expression: string; time_zone: string }
  | { kind: "interval"; every_seconds: number };
export type ThreadMode = { kind: "new" } | { kind: "fixed"; thread_id: string };
export type Halt =
  | "paused"
  | "failures"
  | "registrant_left"
  | "agent_version_unusable"
  | "thread_deleted"
  | "tenant_deleted"
  | "interval_too_short";
export type Schedule = {
  id: string;
  workspace_id: string;
  registrant: string;
  mine: boolean;
  title: string;
  request: string;
  agent: { id: string; version: string };
  trigger: Trigger;
  thread: ThreadMode;
  approval_mode: ApprovalMode | null;
  max_lateness_seconds: number;
  status: "active" | "paused" | "stopped";
  halt: Halt | null;
  halted_at: string | null;
  halt_reason: string | null;
  successor: string | null;
  next_scheduled_at: string | null;
  consecutive_failures: number;
  created_by_run_id: string | null;
  revision: number;
  created_at: string;
  updated_at: string;
  /** What the viewer may do: edit, pause or delete, resume. */
  may_edit: boolean;
  may_manage: boolean;
  may_resume: boolean;
};
export type Tick = {
  id: string;
  schedule_id: string;
  scheduled_at: string;
  recorded_at: string;
  status: "started" | "completed" | "failed" | "cancelled" | "skipped";
  reason: string | null;
  task_id: string | null;
  thread_id: string | null;
  started_by: string | null;
};
export type ScheduleDetail = {
  schedule: Schedule;
  ticks: Tick[];
  newer_agent_version: string | null;
  may_edit: boolean;
  may_administer: boolean;
};
export type ScheduleNotice = {
  id: string;
  workspace_id: string;
  title: string;
  registrant: string;
  halt: Halt;
  halted_at: string | null;
  halt_reason: string | null;
  successor: string | null;
};
export type ScheduleNotices = {
  stopped: ScheduleNotice[];
  scheduled_tasks: string[];
};
export type Limits = {
  max_schedules: number;
  max_per_person: number;
  min_interval_seconds: number;
};
/** The Tenant's limits as set, the Deployment's ceilings, and the limits
 *  that apply now after the ceilings and any Tier. */
export type LimitsView = {
  limits: Limits;
  ceilings: Limits;
  effective: Limits;
};

/** What a person fills in to register or change a Schedule. */
export type ScheduleForm = {
  title: string;
  request: string;
  agentId: string;
  agentVersion: string;
  timing: "cron" | "interval";
  expression: string;
  timeZone: string;
  everyMinutes: string;
  thread: string;
  approvalMode: ApprovalMode | "default";
  maxLatenessMinutes: string;
};
export type FormProblem =
  | "title"
  | "request"
  | "agent"
  | "expression"
  | "time_zone"
  | "interval"
  | "lateness";

export function emptyForm(timeZone: string): ScheduleForm {
  return {
    title: "",
    request: "",
    agentId: "",
    agentVersion: "",
    timing: "cron",
    expression: "0 9 * * 1-5",
    timeZone,
    everyMinutes: "60",
    thread: "",
    approvalMode: "default",
    maxLatenessMinutes: "60",
  };
}

export function formOf(schedule: Schedule): ScheduleForm {
  return {
    title: schedule.title,
    request: schedule.request,
    agentId: schedule.agent.id,
    agentVersion: schedule.agent.version,
    timing: schedule.trigger.kind,
    expression:
      schedule.trigger.kind === "cron" ? schedule.trigger.expression : "",
    timeZone:
      schedule.trigger.kind === "cron" ? schedule.trigger.time_zone : "UTC",
    everyMinutes:
      schedule.trigger.kind === "interval"
        ? String(Math.round(schedule.trigger.every_seconds / 60))
        : "60",
    thread: schedule.thread.kind === "fixed" ? schedule.thread.thread_id : "",
    approvalMode: schedule.approval_mode ?? "default",
    maxLatenessMinutes: String(Math.round(schedule.max_lateness_seconds / 60)),
  };
}

const CRON_FIELD = /^[\w*,/-]+$/;

/** The fields to fix before sending, checked as far as the browser can; the
 *  server checks the rest, such as the Tenant's minimum interval. */
export function formProblems(
  form: ScheduleForm,
  minIntervalSeconds: number,
): FormProblem[] {
  const problems: FormProblem[] = [];
  if (!form.title.trim() || form.title.trim().length > 200)
    problems.push("title");
  if (!form.request.trim() || form.request.length > 16000)
    problems.push("request");
  if (!form.agentId.trim() || !form.agentVersion.trim()) problems.push("agent");
  if (form.timing === "cron") {
    const fields = form.expression.trim().split(/\s+/);
    if (fields.length !== 5 || !fields.every((field) => CRON_FIELD.test(field)))
      problems.push("expression");
    if (!form.timeZone.trim()) problems.push("time_zone");
  } else {
    const minutes = Number(form.everyMinutes);
    if (
      !Number.isInteger(minutes) ||
      minutes * 60 < minIntervalSeconds ||
      minutes > 366 * 24 * 60
    )
      problems.push("interval");
  }
  const lateness = Number(form.maxLatenessMinutes);
  if (!Number.isInteger(lateness) || lateness < 1 || lateness > 7 * 24 * 60)
    problems.push("lateness");
  return problems;
}

/** The body of `POST /api/workspaces/{workspace}/schedules`. */
export function definitionOf(form: ScheduleForm) {
  return {
    title: form.title.trim(),
    request: form.request,
    agent: { id: form.agentId.trim(), version: form.agentVersion.trim() },
    trigger:
      form.timing === "cron"
        ? {
            kind: "cron" as const,
            expression: form.expression.trim().replace(/\s+/g, " "),
            time_zone: form.timeZone.trim(),
          }
        : {
            kind: "interval" as const,
            every_seconds: Number(form.everyMinutes) * 60,
          },
    thread: form.thread.trim()
      ? { kind: "fixed" as const, thread_id: form.thread.trim() }
      : { kind: "new" as const },
    ...(form.approvalMode === "default"
      ? {}
      : { approval_mode: form.approvalMode }),
    max_lateness_seconds: Number(form.maxLatenessMinutes) * 60,
  };
}

/** The body of `PATCH /api/schedules/{id}`: the whole form, with the
 *  Registrant's default mode named explicitly. */
export function patchOf(form: ScheduleForm) {
  const definition = definitionOf(form);
  return {
    title: definition.title,
    request: definition.request,
    agent: definition.agent,
    trigger: definition.trigger,
    thread: definition.thread,
    max_lateness_seconds: definition.max_lateness_seconds,
    approval_mode: form.approvalMode,
  };
}

/** A scheduled time in the viewer's time zone. */
export function formatTime(
  iso: string | null,
  locale: string,
  timeZone?: string,
): string {
  if (!iso) return "—";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
    ...(timeZone ? { timeZone } : {}),
  }).format(new Date(iso));
}

export function triggerSummary(trigger: Trigger, ja: boolean): string {
  if (trigger.kind === "cron")
    return `${trigger.expression} (${trigger.time_zone})`;
  const minutes = trigger.every_seconds / 60;
  if (minutes % 1440 === 0)
    return ja ? `${minutes / 1440}日ごと` : `every ${minutes / 1440} d`;
  if (minutes % 60 === 0)
    return ja ? `${minutes / 60}時間ごと` : `every ${minutes / 60} h`;
  return ja ? `${minutes}分ごと` : `every ${minutes} min`;
}

export const haltCopy: Record<Halt, { ja: string; en: string }> = {
  paused: { ja: "一時停止中", en: "Paused" },
  failures: {
    ja: "3回続けて失敗したため一時停止しました",
    en: "Paused after 3 failed Runs in a row",
  },
  registrant_left: {
    ja: "登録者が離れたため停止しました。後継者を待っています",
    en: "Stopped: its Registrant left. Waiting for a successor",
  },
  agent_version_unusable: {
    ja: "Agent のバージョンが使えなくなったため停止しました",
    en: "Stopped: its Agent version can no longer be started",
  },
  thread_deleted: {
    ja: "固定スレッドが削除されたため停止しました",
    en: "Stopped: its Thread was deleted",
  },
  tenant_deleted: {
    ja: "Tenant が削除されたため停止しました",
    en: "Stopped: the Tenant was deleted",
  },
  interval_too_short: {
    ja: "Tenant の最短間隔より短いため停止しました。間隔を変えて再開してください",
    en: "Stopped: shorter than the Tenant's minimum interval. Change it and resume",
  },
};

/** "skipped (limit)" and the like, for the tick history. */
export function tickLabel(tick: Tick, ja: boolean): string {
  const status = {
    started: ja ? "実行中" : "started",
    completed: ja ? "完了" : "completed",
    failed: ja ? "失敗" : "failed",
    cancelled: ja ? "取消" : "cancelled",
    skipped: ja ? "スキップ" : "skipped",
  }[tick.status];
  const reason = tick.reason?.split(":")[0];
  const reasons: Record<string, string> = ja
    ? {
        overlap: "前回が実行中",
        late: "遅すぎた",
        limit: "利用上限",
        interval: "前回から短すぎる",
      }
    : {
        overlap: "overlap",
        late: "late",
        limit: "limit",
        interval: "too soon",
      };
  const shown =
    tick.status === "skipped" && reason
      ? (reasons[reason] ?? tick.reason)
      : tick.reason;
  const manual = tick.started_by
    ? ja
      ? ` · ${tick.started_by} が今すぐ実行`
      : ` · run now by ${tick.started_by}`
    : "";
  return `${status}${shown ? ` (${shown})` : ""}${manual}`;
}

/** Bell entries for stopped Schedules (#557 decision 25). They wait in the
 *  pending section until someone resumes or deletes the Schedule. */
export function scheduleNoticeEntries(
  node: string,
  notices: ScheduleNotices,
  workspaceTitle: (id: string) => string,
  ja: boolean,
): { scope: string; entries: NoticeEntry[] } {
  const scope = JSON.stringify([node, "schedules"]);
  return {
    scope,
    // Read defensively: a Node without Schedules (or a test double) may
    // answer with something other than a ScheduleNotices object, and a throw
    // here would take the whole dashboard down with the bell.
    entries: (Array.isArray(notices?.stopped) ? notices.stopped : []).map(
      (notice) => ({
        key: JSON.stringify([node, "schedule", notice.id]),
        scope,
        signature: `${notice.halt}:${notice.halted_at ?? ""}`,
        kind: "schedule_stopped",
        detail:
          `${notice.title} — ${haltText(notice.halt, notice.halt_reason, ja)}`.slice(
            0,
            180,
          ),
        workspaceTitle: workspaceTitle(notice.workspace_id),
        target: {
          kind: "schedule",
          id: notice.id,
          node,
          workspace: notice.workspace_id,
        },
      }),
    ),
  };
}

/** Whether a Task entry's notice stays out of the toast: a scheduled Run's
 *  completion is shown in the bell only; its failure still toasts. */
export function quietCompletion(
  kind: NoticeEntry["kind"],
  task: string,
  scheduled: ReadonlySet<string>,
): boolean {
  return kind === "completed" && scheduled.has(task);
}

/** Which actions a Schedule's row offers the viewer. The server decides
 *  the `may_*` flags from the same rules it enforces: only the Registrant
 *  edits or resumes; the Registrant or a holder of `schedule.administer`
 *  pauses and deletes; only the named successor takes a departed
 *  Registrant's Schedule over. */
export function scheduleActions(schedule: Schedule): {
  edit: boolean;
  pause: boolean;
  resume: "resume" | "takeOver" | null;
  remove: boolean;
} {
  return {
    edit: schedule.may_edit,
    pause: schedule.status === "active" && schedule.may_manage,
    resume:
      schedule.status !== "active" && schedule.may_resume
        ? schedule.mine
          ? "resume"
          : "takeOver"
        : null,
    remove: schedule.may_manage,
  };
}

/** Why a Schedule stopped, with the server's words when it gave them. */
export function haltText(
  halt: Halt,
  reason: string | null,
  ja: boolean,
): string {
  const text = haltCopy[halt][ja ? "ja" : "en"];
  return reason ? `${text} (${reason})` : text;
}
