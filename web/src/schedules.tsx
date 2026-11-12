/** Schedules (#557): a Workspace's Schedules, "my Schedules", and for holders
 *  of `schedule.administer` every Schedule of the Tenant. A Schedule always
 *  acts as its Registrant (ADR 0134), so only they edit or resume it. */
import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import { Field, Panel, useI18n } from "./ui";
import { Alert, Hint, Loading } from "./components/patterns";
import { Badge } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import {
  definitionOf,
  emptyForm,
  formOf,
  formProblems,
  formatTime,
  haltCopy,
  patchOf,
  tickLabel,
  triggerSummary,
  type FormProblem,
  type LimitsView,
  type Schedule,
  type ScheduleDetail,
  type ScheduleForm,
} from "./schedules-model";

const copy = {
  "ja-JP": {
    title: "スケジュール",
    mine: "自分のスケジュール",
    tenant: "Tenant のすべてのスケジュール",
    hint: "スケジュールは登録した人として実行されます。その人の承認モード・承認ルール・接続が適用されます。",
    empty: "スケジュールはありません。",
    create: "スケジュールを登録",
    edit: "変更",
    save: "保存",
    cancel: "キャンセル",
    pause: "一時停止",
    resume: "再開",
    takeOver: "引き継いで再開",
    remove: "削除",
    runNow: "今すぐ実行",
    history: "履歴",
    next: "次回",
    registrant: "登録者",
    agent: "Agent",
    agentId: "Agent ID",
    agentVersion: "バージョン",
    newer: (version: string) => `新しいバージョン ${version} があります`,
    request: "依頼内容",
    name: "名前",
    timing: "タイミング",
    cron: "cron 式",
    interval: "一定間隔",
    expression: "cron 式（分 時 日 月 曜日）",
    timeZone: "タイムゾーン",
    everyMinutes: "間隔（分）",
    thread: "固定スレッド ID（空欄なら毎回新しいスレッド）",
    mode: "承認モード",
    defaultMode: "自分の既定",
    lateness: "許容する遅れ（分）",
    successor: "後継者のサブジェクト",
    nameSuccessor: "後継者を指名",
    confirmDelete: "このスケジュールを削除しますか？",
    problems: {
      title: "名前を入力してください（200文字以内）",
      request: "依頼内容を入力してください",
      agent: "Agent とバージョンを入力してください",
      expression: "cron 式は5つのフィールドで書いてください",
      time_zone: "タイムゾーンを入力してください",
      interval: "間隔が短すぎるか長すぎます",
      lateness: "許容する遅れは1分から7日までです",
    } satisfies Record<FormProblem, string>,
    status: { active: "有効", paused: "一時停止", stopped: "停止" },
    byAgent: "Agent が登録",
    limits: (limits: LimitsView["limits"]) =>
      `上限: Tenant ${limits.max_schedules}件・1人 ${limits.max_per_person}件・最短 ${limits.min_interval_seconds / 60}分`,
  },
  "en-US": {
    title: "Schedules",
    mine: "My Schedules",
    tenant: "All Schedules of the Tenant",
    hint: "A Schedule runs as the person who registered it, with their Approval Mode, Approval Rules and connections.",
    empty: "No Schedules.",
    create: "Register a Schedule",
    edit: "Edit",
    save: "Save",
    cancel: "Cancel",
    pause: "Pause",
    resume: "Resume",
    takeOver: "Take over and resume",
    remove: "Delete",
    runNow: "Run now",
    history: "History",
    next: "Next",
    registrant: "Registrant",
    agent: "Agent",
    agentId: "Agent ID",
    agentVersion: "Version",
    newer: (version: string) => `Version ${version} is available`,
    request: "Request",
    name: "Name",
    timing: "Timing",
    cron: "Cron expression",
    interval: "Fixed interval",
    expression: "Cron expression (minute hour day month weekday)",
    timeZone: "Time zone",
    everyMinutes: "Every (minutes)",
    thread: "Fixed Thread ID (empty: a new Thread every time)",
    mode: "Approval Mode",
    defaultMode: "My default",
    lateness: "Maximum lateness (minutes)",
    successor: "Successor subject",
    nameSuccessor: "Name successor",
    confirmDelete: "Delete this Schedule?",
    problems: {
      title: "Enter a name of up to 200 characters",
      request: "Enter the request",
      agent: "Enter the Agent and its version",
      expression: "A cron expression has five fields",
      time_zone: "Enter a time zone",
      interval: "The interval is too short or too long",
      lateness: "Lateness is between 1 minute and 7 days",
    } satisfies Record<FormProblem, string>,
    status: { active: "Active", paused: "Paused", stopped: "Stopped" },
    byAgent: "Registered by an Agent",
    limits: (limits: LimitsView["limits"]) =>
      `Limits: ${limits.max_schedules} per Tenant, ${limits.max_per_person} per person, every ${limits.min_interval_seconds / 60} min at most`,
  },
};

type Scope =
  | { kind: "workspace"; workspace: string }
  | { kind: "mine" }
  | { kind: "tenant" };

const json = (body: unknown): RequestInit => ({
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify(body),
});

function listUrl(scope: Scope): string {
  if (scope.kind === "workspace")
    return `/api/workspaces/${encodeURIComponent(scope.workspace)}/schedules`;
  return `/api/schedules?scope=${scope.kind}`;
}

export function SchedulesPanel({
  scope,
  administrator = false,
}: {
  scope: Scope;
  /** The viewer holds `schedule.administer` and may list the whole Tenant. */
  administrator?: boolean;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [all, setAll] = useState(false);
  const shown: Scope =
    scope.kind === "mine" && all ? { kind: "tenant" } : scope;
  const key = ["schedules", shown];
  const list = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<Schedule[]>(listUrl(shown)),
    retry: false,
  });
  const limits = useQuery({
    queryKey: ["schedule-limits"],
    queryFn: () => apiFetch<LimitsView>("/api/schedules/limits"),
    retry: false,
  });
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<Schedule | "new" | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const timeZone = useMemo(
    () => Intl.DateTimeFormat().resolvedOptions().timeZone,
    [],
  );
  async function act(run: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await run();
      await client.invalidateQueries({ queryKey: ["schedules"] });
      await client.invalidateQueries({ queryKey: ["schedule"] });
      await client.invalidateQueries({ queryKey: ["schedule-notices"] });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  const minInterval = limits.data?.limits.min_interval_seconds ?? 900;
  return (
    <Panel title={scope.kind === "workspace" ? words.title : words.mine}>
      <Hint>{words.hint}</Hint>
      {limits.data && <Hint>{words.limits(limits.data.limits)}</Hint>}
      {scope.kind === "mine" && administrator && (
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={all}
            onChange={(event) => setAll(event.target.checked)}
          />
          {words.tenant}
        </label>
      )}
      {error && <Alert>{error}</Alert>}
      {list.isError && <Alert>{String(list.error)}</Alert>}
      {list.isLoading && <Loading />}
      {list.data?.length === 0 && <Hint>{words.empty}</Hint>}
      <ul className="divide-y divide-border">
        {list.data?.map((schedule) => (
          <li key={schedule.id} className="grid gap-2 py-3">
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                className="text-left text-sm font-medium underline-offset-2 hover:underline"
                onClick={() =>
                  setSelected(selected === schedule.id ? null : schedule.id)
                }
              >
                {schedule.title}
              </button>
              <Badge
                variant={schedule.status === "active" ? "secondary" : "outline"}
              >
                {words.status[schedule.status]}
              </Badge>
              {schedule.created_by_run_id && (
                <Badge variant="outline">{words.byAgent}</Badge>
              )}
            </div>
            <div className="text-xs text-muted-foreground">
              {triggerSummary(schedule.trigger, ja)} · {words.next}:{" "}
              {formatTime(schedule.next_scheduled_at, locale, timeZone)} ·{" "}
              {words.agent}: {schedule.agent.id}@{schedule.agent.version}
              {!schedule.mine &&
                ` · ${words.registrant}: ${schedule.registrant}`}
            </div>
            {schedule.halt && schedule.halt !== "paused" && (
              <Hint>{haltCopy[schedule.halt][ja ? "ja" : "en"]}</Hint>
            )}
            <div className="flex flex-wrap gap-2">
              {schedule.mine && schedule.halt !== "registrant_left" && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => setEditing(schedule)}
                >
                  {words.edit}
                </Button>
              )}
              {schedule.status === "active" ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() =>
                    void act(() =>
                      apiFetch(`/api/schedules/${schedule.id}/pause`, json({})),
                    )
                  }
                >
                  {words.pause}
                </Button>
              ) : (
                (schedule.mine || schedule.successor !== null) && (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={() =>
                      void act(() =>
                        apiFetch(
                          `/api/schedules/${schedule.id}/resume`,
                          json({}),
                        ),
                      )
                    }
                  >
                    {schedule.mine ? words.resume : words.takeOver}
                  </Button>
                )
              )}
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void act(() =>
                    apiFetch(`/api/schedules/${schedule.id}/run-now`, json({})),
                  )
                }
              >
                {words.runNow}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => {
                  if (window.confirm(words.confirmDelete))
                    void act(() =>
                      apiFetch(`/api/schedules/${schedule.id}`, {
                        method: "DELETE",
                      }),
                    );
                }}
              >
                {words.remove}
              </Button>
            </div>
            {selected === schedule.id && (
              <ScheduleHistory
                id={schedule.id}
                timeZone={timeZone}
                onSuccessor={(subject) =>
                  void act(() =>
                    apiFetch(
                      `/api/schedules/${schedule.id}/successor`,
                      json({ subject }),
                    ),
                  )
                }
              />
            )}
          </li>
        ))}
      </ul>
      {editing ? (
        <ScheduleEditor
          key={editing === "new" ? "new" : editing.id}
          initial={editing === "new" ? emptyForm(timeZone) : formOf(editing)}
          minInterval={minInterval}
          busy={busy}
          onCancel={() => setEditing(null)}
          onSave={(form) =>
            void act(async () => {
              if (editing === "new") {
                if (scope.kind !== "workspace") return;
                await apiFetch(listUrl(scope), json(definitionOf(form)));
              } else {
                await apiFetch(`/api/schedules/${editing.id}`, {
                  method: "PATCH",
                  headers: { "content-type": "application/json" },
                  body: JSON.stringify(patchOf(form)),
                });
              }
              setEditing(null);
            })
          }
        />
      ) : (
        scope.kind === "workspace" && (
          <Button
            className="justify-self-start"
            disabled={busy}
            onClick={() => setEditing("new")}
          >
            {words.create}
          </Button>
        )
      )}
    </Panel>
  );
}

function ScheduleHistory({
  id,
  timeZone,
  onSuccessor,
}: {
  id: string;
  timeZone: string;
  onSuccessor: (subject: string) => void;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const ja = locale === "ja-JP";
  const [successor, setSuccessor] = useState("");
  const detail = useQuery({
    queryKey: ["schedule", id],
    queryFn: () => apiFetch<ScheduleDetail>(`/api/schedules/${id}`),
    retry: false,
  });
  if (detail.isError) return <Alert>{String(detail.error)}</Alert>;
  if (!detail.data) return <Loading />;
  const { schedule, ticks, newer_agent_version, may_administer } = detail.data;
  return (
    <div className="grid gap-2 rounded-md border border-border p-3">
      <p className="whitespace-pre-wrap text-sm">{schedule.request}</p>
      {newer_agent_version && schedule.mine && (
        <Hint>{words.newer(newer_agent_version)}</Hint>
      )}
      {may_administer && schedule.halt === "registrant_left" && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (successor.trim()) onSuccessor(successor.trim());
          }}
        >
          <Field label={words.successor}>
            <Input
              value={successor}
              onChange={(event) => setSuccessor(event.target.value)}
            />
          </Field>
          <Button size="sm" type="submit">
            {words.nameSuccessor}
          </Button>
        </form>
      )}
      <h4 className="text-xs font-medium text-faint">{words.history}</h4>
      <ul className="grid gap-1 text-xs">
        {ticks.map((tick) => (
          <li key={tick.id} className="flex flex-wrap gap-2">
            <span className="font-mono tabular">
              {formatTime(tick.scheduled_at, locale, timeZone)}
            </span>
            <span>{tickLabel(tick, ja)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ScheduleEditor({
  initial,
  minInterval,
  busy,
  onCancel,
  onSave,
}: {
  initial: ScheduleForm;
  minInterval: number;
  busy: boolean;
  onCancel: () => void;
  onSave: (form: ScheduleForm) => void;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const [form, setForm] = useState(initial);
  const [tried, setTried] = useState(false);
  const problems = formProblems(form, minInterval);
  const set = <K extends keyof ScheduleForm>(key: K, value: ScheduleForm[K]) =>
    setForm((current) => ({ ...current, [key]: value }));
  return (
    <form
      className="grid gap-3 rounded-md border border-border p-3"
      onSubmit={(event) => {
        event.preventDefault();
        setTried(true);
        if (!problems.length) onSave(form);
      }}
    >
      <Field label={words.name}>
        <Input
          value={form.title}
          onChange={(event) => set("title", event.target.value)}
        />
      </Field>
      <Field label={words.request}>
        <Textarea
          value={form.request}
          onChange={(event) => set("request", event.target.value)}
        />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={words.agentId}>
          <Input
            value={form.agentId}
            onChange={(event) => set("agentId", event.target.value)}
          />
        </Field>
        <Field label={words.agentVersion}>
          <Input
            value={form.agentVersion}
            onChange={(event) => set("agentVersion", event.target.value)}
          />
        </Field>
      </div>
      <Field label={words.timing}>
        <NativeSelect
          value={form.timing}
          onChange={(event) =>
            set("timing", event.target.value as ScheduleForm["timing"])
          }
        >
          <option value="cron">{words.cron}</option>
          <option value="interval">{words.interval}</option>
        </NativeSelect>
      </Field>
      {form.timing === "cron" ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={words.expression}>
            <Input
              className="font-mono"
              value={form.expression}
              onChange={(event) => set("expression", event.target.value)}
            />
          </Field>
          <Field label={words.timeZone}>
            <Input
              value={form.timeZone}
              onChange={(event) => set("timeZone", event.target.value)}
            />
          </Field>
        </div>
      ) : (
        <Field label={words.everyMinutes}>
          <Input
            inputMode="numeric"
            value={form.everyMinutes}
            onChange={(event) => set("everyMinutes", event.target.value)}
          />
        </Field>
      )}
      <Field label={words.thread}>
        <Input
          className="font-mono"
          value={form.thread}
          onChange={(event) => set("thread", event.target.value)}
        />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={words.mode}>
          <NativeSelect
            value={form.approvalMode}
            onChange={(event) =>
              set(
                "approvalMode",
                event.target.value as ScheduleForm["approvalMode"],
              )
            }
          >
            <option value="default">{words.defaultMode}</option>
            <option value="manual">Manual</option>
            <option value="auto">Auto</option>
            <option value="dont_ask">Don't ask</option>
            <option value="bypass">Bypass</option>
          </NativeSelect>
        </Field>
        <Field label={words.lateness}>
          <Input
            inputMode="numeric"
            value={form.maxLatenessMinutes}
            onChange={(event) => set("maxLatenessMinutes", event.target.value)}
          />
        </Field>
      </div>
      {tried && problems.length > 0 && (
        <Alert>
          {problems.map((problem) => words.problems[problem]).join(" / ")}
        </Alert>
      )}
      <div className="flex gap-2">
        <Button type="submit" disabled={busy}>
          {words.save}
        </Button>
        <Button type="button" variant="ghost" onClick={onCancel}>
          {words.cancel}
        </Button>
      </div>
    </form>
  );
}
