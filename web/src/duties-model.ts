/** Duties (#1011, ADR 0142; from the Schedules of #557): what the API
 *  returns and the pure rules the Duty screens and the notification bell
 *  share. */
import type { NoticeEntry } from "./notifications/model";

export type ApprovalMode = "manual" | "auto" | "dont_ask" | "bypass";
export type Trigger =
  | { kind: "cron"; expression: string; time_zone: string }
  | { kind: "interval"; every_seconds: number };
/** A Router's or Watcher's code (#1011): Python or shell that runs without
 *  a model. */
export type ScriptLanguage = "python" | "shell";
export type Script = { language: ScriptLanguage; source: string };
export type Watcher = { script: Script; every_seconds: number };
/** What a Router decided about one event. */
export type RouterAction =
  | { action: "drop" }
  | { action: "act" }
  | { action: "task"; title: string; description?: string }
  | { action: "deliver"; task_id: string; input: string };
export type DutyEvent = {
  id: string;
  received_at: string;
  outcome:
    | "started"
    | "delivered"
    | "skipped"
    | "failed"
    | "routing"
    | "dropped"
    | "acted";
  reason: string | null;
  task_id?: string | null;
  router?: RouterAction | null;
};
/** One saved version of a Duty's script, with who saved it and when. */
export type ScriptVersion = {
  id: string;
  duty_id: string;
  role: "router" | "watcher";
  language: ScriptLanguage;
  source: string;
  every_seconds: number | null;
  author: string;
  created_at: string;
};
export type Halt =
  | "paused"
  | "failures"
  | "requester_left"
  | "agent_version_unusable"
  | "thread_deleted"
  | "tenant_deleted";
export type Duty = {
  id: string;
  /** The Duty's own Thread, where its Tasks belong (#1008). */
  thread_id: string;
  /** The Thread it was registered from; null before #1008. */
  origin_thread_id: string | null;
  requester: string;
  mine: boolean;
  title: string;
  /** What the Duty is for. */
  mission: string;
  /** The checklist its Agent follows on every Task. */
  procedure: string;
  agent: { id: string; version: string };
  /** When it wakes by the clock, if it does. */
  trigger: Trigger | null;
  approval_mode: ApprovalMode | null;
  max_lateness_seconds: number;
  /** Failed Tasks in a row after which it pauses itself; 0 never does. */
  pause_after_failures: number;
  /** Decides what each event no Task booked does, instead of one Task per
   *  event (#1011). */
  router?: Script | null;
  /** Polls an outside service on its own interval and reports events. */
  watcher?: Watcher | null;
  watcher_next_at?: string | null;
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
  duty_id: string;
  scheduled_at: string;
  recorded_at: string;
  status: "started" | "completed" | "failed" | "cancelled" | "skipped";
  reason: string | null;
  task_id: string | null;
  started_by: string | null;
};
export type DutyDetail = {
  duty: Duty;
  ticks: Tick[];
  newer_agent_version: string | null;
  /** Saved Router and Watcher versions, newest first. */
  script_versions?: ScriptVersion[];
  may_edit: boolean;
  may_administer: boolean;
};
export type DutyNotice = {
  id: string;
  thread_id: string;
  title: string;
  requester: string;
  halt: Halt;
  halted_at: string | null;
  halt_reason: string | null;
  successor: string | null;
};
export type DutyNotices = {
  stopped: DutyNotice[];
  scheduled_tasks: string[];
};

/** What a person fills in to register or change a Duty. */
export type DutyForm = {
  title: string;
  mission: string;
  procedure: string;
  agentId: string;
  agentVersion: string;
  /** "none" for a Duty that wakes only on its other triggers. */
  timing: "cron" | "interval" | "none";
  expression: string;
  timeZone: string;
  everyMinutes: string;
  approvalMode: ApprovalMode | "default";
  maxLatenessMinutes: string;
  pauseAfterFailures: string;
  /** The Router, when the Duty has one. */
  routerOn: boolean;
  routerLanguage: ScriptLanguage;
  routerSource: string;
  /** The Watcher, when the Duty has one. */
  watcherOn: boolean;
  watcherLanguage: ScriptLanguage;
  watcherSource: string;
  watcherEveryMinutes: string;
};
export type FormProblem =
  | "router"
  | "watcher"
  | "watcher_interval"
  | "title"
  | "mission"
  | "procedure"
  | "agent"
  | "expression"
  | "time_zone"
  | "interval"
  | "lateness"
  | "failures";

export function emptyForm(timeZone: string): DutyForm {
  return {
    title: "",
    mission: "",
    procedure: "",
    agentId: "",
    agentVersion: "",
    timing: "cron",
    expression: "0 9 * * 1-5",
    timeZone,
    everyMinutes: "60",
    approvalMode: "default",
    maxLatenessMinutes: "60",
    pauseAfterFailures: "3",
    routerOn: false,
    routerLanguage: "python",
    routerSource: "",
    watcherOn: false,
    watcherLanguage: "python",
    watcherSource: "",
    watcherEveryMinutes: "5",
  };
}

export function formOf(duty: Duty): DutyForm {
  return {
    title: duty.title,
    mission: duty.mission,
    procedure: duty.procedure,
    agentId: duty.agent.id,
    agentVersion: duty.agent.version,
    timing: duty.trigger?.kind ?? "none",
    expression: duty.trigger?.kind === "cron" ? duty.trigger.expression : "",
    timeZone: duty.trigger?.kind === "cron" ? duty.trigger.time_zone : "UTC",
    everyMinutes:
      duty.trigger?.kind === "interval"
        ? String(Math.round(duty.trigger.every_seconds / 60))
        : "60",
    approvalMode: duty.approval_mode ?? "default",
    maxLatenessMinutes: String(Math.round(duty.max_lateness_seconds / 60)),
    pauseAfterFailures: String(duty.pause_after_failures),
    routerOn: Boolean(duty.router),
    routerLanguage: duty.router?.language ?? "python",
    routerSource: duty.router?.source ?? "",
    watcherOn: Boolean(duty.watcher),
    watcherLanguage: duty.watcher?.script.language ?? "python",
    watcherSource: duty.watcher?.script.source ?? "",
    watcherEveryMinutes: duty.watcher
      ? String(Math.round(duty.watcher.every_seconds / 60))
      : "5",
  };
}

/** The largest script source, in bytes, as the server counts it. */
export const MAX_SCRIPT_BYTES = 64 * 1024;

function scriptProblem(source: string): boolean {
  return (
    !source.trim() ||
    source.includes("\0") ||
    new TextEncoder().encode(source).length > MAX_SCRIPT_BYTES
  );
}

const CRON_FIELD = /^[\w*,/-]+$/;

/** The fields to fix before sending, checked as far as the browser can; the
 *  server checks the rest. There is no minimum interval (#1012). */
export function formProblems(form: DutyForm): FormProblem[] {
  const problems: FormProblem[] = [];
  if (!form.title.trim() || form.title.trim().length > 200)
    problems.push("title");
  if (!form.mission.trim() || form.mission.length > 16000)
    problems.push("mission");
  if (form.procedure.length > 16000) problems.push("procedure");
  if (!form.agentId.trim() || !form.agentVersion.trim()) problems.push("agent");
  if (form.timing === "cron") {
    const fields = form.expression.trim().split(/\s+/);
    if (fields.length !== 5 || !fields.every((field) => CRON_FIELD.test(field)))
      problems.push("expression");
    if (!form.timeZone.trim()) problems.push("time_zone");
  } else if (form.timing === "interval") {
    const minutes = Number(form.everyMinutes);
    if (!Number.isInteger(minutes) || minutes < 1 || minutes > 366 * 24 * 60)
      problems.push("interval");
  }
  const failures = Number(form.pauseAfterFailures);
  if (!Number.isInteger(failures) || failures < 0 || failures > 1000)
    problems.push("failures");
  const lateness = Number(form.maxLatenessMinutes);
  if (!Number.isInteger(lateness) || lateness < 1 || lateness > 7 * 24 * 60)
    problems.push("lateness");
  if (form.routerOn && scriptProblem(form.routerSource))
    problems.push("router");
  if (form.watcherOn) {
    if (scriptProblem(form.watcherSource)) problems.push("watcher");
    // A Watcher runs every minute at most often, and at least once a year.
    const minutes = Number(form.watcherEveryMinutes);
    if (!Number.isInteger(minutes) || minutes < 1 || minutes > 366 * 24 * 60)
      problems.push("watcher_interval");
  }
  return problems;
}

function routerOf(form: DutyForm): Script | undefined {
  return form.routerOn
    ? { language: form.routerLanguage, source: form.routerSource }
    : undefined;
}

function watcherOf(form: DutyForm): Watcher | undefined {
  return form.watcherOn
    ? {
        script: { language: form.watcherLanguage, source: form.watcherSource },
        every_seconds: Number(form.watcherEveryMinutes) * 60,
      }
    : undefined;
}

/** The body of `POST /api/threads/{thread}/duties`. */
export function definitionOf(form: DutyForm) {
  return {
    title: form.title.trim(),
    mission: form.mission,
    procedure: form.procedure,
    agent: { id: form.agentId.trim(), version: form.agentVersion.trim() },
    ...(timeTrigger(form) ? { trigger: timeTrigger(form) } : {}),
    ...(form.approvalMode === "default"
      ? {}
      : { approval_mode: form.approvalMode }),
    max_lateness_seconds: Number(form.maxLatenessMinutes) * 60,
    pause_after_failures: Number(form.pauseAfterFailures),
    ...(routerOf(form) ? { router: routerOf(form) } : {}),
    ...(watcherOf(form) ? { watcher: watcherOf(form) } : {}),
  };
}

function timeTrigger(form: DutyForm): Trigger | undefined {
  if (form.timing === "cron")
    return {
      kind: "cron",
      expression: form.expression.trim().replace(/\s+/g, " "),
      time_zone: form.timeZone.trim(),
    };
  if (form.timing === "interval")
    return { kind: "interval", every_seconds: Number(form.everyMinutes) * 60 };
  return undefined;
}

/** The body of `PATCH /api/duties/{id}`: the whole form, with the
 *  Requester's default mode named explicitly. The form counts in whole
 *  minutes; a field left as `original` showed it keeps its exact seconds, so
 *  saving a Duty registered in seconds does not retime it. */
export function patchOf(form: DutyForm, original?: Duty) {
  const definition = definitionOf(form);
  const shown = original ? formOf(original) : undefined;
  const sameInterval =
    original?.trigger?.kind === "interval" &&
    form.timing === "interval" &&
    form.everyMinutes === shown?.everyMinutes;
  const sameLateness = form.maxLatenessMinutes === shown?.maxLatenessMinutes;
  const watcher = watcherOf(form);
  // A Watcher kept in seconds keeps them when its minutes were not touched.
  const sameWatcherInterval =
    watcher &&
    original?.watcher &&
    form.watcherEveryMinutes === shown?.watcherEveryMinutes;
  return {
    title: definition.title,
    mission: definition.mission,
    procedure: definition.procedure,
    agent: definition.agent,
    trigger: sameInterval
      ? original.trigger
      : (timeTrigger(form) ?? { kind: "none" as const }),
    pause_after_failures: definition.pause_after_failures,
    max_lateness_seconds:
      sameLateness && original
        ? original.max_lateness_seconds
        : definition.max_lateness_seconds,
    approval_mode: form.approvalMode,
    router: routerOf(form) ?? { kind: "none" as const },
    watcher: watcher
      ? sameWatcherInterval
        ? {
            ...watcher,
            every_seconds:
              original?.watcher?.every_seconds ?? watcher.every_seconds,
          }
        : watcher
      : { kind: "none" as const },
  };
}

/** What became of an event, with the Router's decision when it made one. */
export function eventLabel(event: DutyEvent, ja: boolean): string {
  const outcome = {
    started: ja ? "仕事を開始" : "started a Task",
    delivered: ja ? "仕事に渡した" : "delivered to a Task",
    skipped: ja ? "スキップ" : "skipped",
    failed: ja ? "失敗" : "failed",
    routing: ja ? "ルーターが判断中" : "Router deciding",
    dropped: ja ? "破棄" : "dropped",
    acted: ja ? "ルーターが処理" : "handled by the Router",
  }[event.outcome];
  const router = event.router ? routerLabel(event.router, ja) : "";
  const reason = event.reason ? ` (${event.reason})` : "";
  return `${outcome}${router ? ` · ${router}` : ""}${reason}`;
}

/** A Router's decision in a few words. */
export function routerLabel(action: RouterAction, ja: boolean): string {
  switch (action.action) {
    case "drop":
      return ja ? "ルーター: 破棄" : "Router: drop";
    case "act":
      return ja ? "ルーター: 自分で処理" : "Router: acted";
    // The Task keeps the Duty's title; the Router's title is its note.
    case "task":
      return ja
        ? `ルーター: 仕事を開始（${action.title}）`
        : `Router: started a Task (${action.title})`;
    case "deliver":
      return ja
        ? `ルーター: 仕事 ${action.task_id} に渡す`
        : `Router: deliver to Task ${action.task_id}`;
  }
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

export function triggerSummary(trigger: Trigger | null, ja: boolean): string {
  if (!trigger) return ja ? "時刻起動なし" : "no time trigger";
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
    ja: "続けて失敗したため一時停止しました",
    en: "Paused after failed Tasks in a row",
  },
  requester_left: {
    ja: "依頼者が離れたため停止しました。後継者を待っています",
    en: "Stopped: its Requester left. Waiting for a successor",
  },
  agent_version_unusable: {
    ja: "Agent のバージョンが使えなくなったため停止しました",
    en: "Stopped: its Agent version can no longer be started",
  },
  thread_deleted: {
    ja: "職務のスレッドが削除されたため停止しました",
    en: "Stopped: its Thread was deleted",
  },
  tenant_deleted: {
    ja: "Tenant が削除されたため停止しました",
    en: "Stopped: the Tenant was deleted",
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

/** Bell entries for stopped Duties (#557 decision 25). They wait in the
 *  pending section until someone resumes or deletes the Duty. */
export function dutyNoticeEntries(
  node: string,
  notices: DutyNotices,
  threadTitle: (id: string) => string,
  ja: boolean,
): { scope: string; entries: NoticeEntry[] } {
  const scope = JSON.stringify([node, "duties"]);
  return {
    scope,
    // Read defensively: a Node without Duties (or a test double) may
    // answer with something other than a DutyNotices object, and a throw
    // here would take the whole dashboard down with the bell.
    entries: (Array.isArray(notices?.stopped) ? notices.stopped : []).map(
      (notice) => ({
        key: JSON.stringify([node, "duty", notice.id]),
        scope,
        signature: `${notice.halt}:${notice.halted_at ?? ""}`,
        kind: "duty_stopped",
        detail:
          `${notice.title} — ${haltText(notice.halt, notice.halt_reason, ja)}`.slice(
            0,
            180,
          ),
        threadTitle: threadTitle(notice.thread_id),
        target: {
          kind: "duty",
          id: notice.id,
          node,
          thread: notice.thread_id,
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

/** Which actions a Duty's row offers the viewer. The server decides
 *  the `may_*` flags from the same rules it enforces: only the Requester
 *  edits or resumes; the Requester or a holder of `duty.administer`
 *  pauses and deletes; only the named successor takes a departed
 *  Requester's Duty over. */
export function dutyActions(duty: Duty): {
  edit: boolean;
  pause: boolean;
  resume: "resume" | "takeOver" | null;
  remove: boolean;
} {
  return {
    edit: duty.may_edit,
    pause: duty.status === "active" && duty.may_manage,
    resume:
      duty.status !== "active" && duty.may_resume
        ? duty.mine
          ? "resume"
          : "takeOver"
        : null,
    remove: duty.may_manage,
  };
}

/** Why a Duty stopped, with the server's words when it gave them. */
export function haltText(
  halt: Halt,
  reason: string | null,
  ja: boolean,
): string {
  const text = haltCopy[halt][ja ? "ja" : "en"];
  return reason ? `${text} (${reason})` : text;
}
