/** Duties (#557): a Workspace's Duties, "my Duties", and for holders
 *  of `duty.administer` every Duty of the Tenant. A Duty always
 *  acts as its Requester (ADR 0134), so only they edit or resume it. */
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
  haltText,
  patchOf,
  dutyActions,
  tickLabel,
  triggerSummary,
  type FormProblem,
  type Duty,
  type DutyDetail,
  type DutyForm,
} from "./duties-model";

const copy = {
  "ja-JP": {
    title: "職務",
    mine: "自分の職務",
    tenant: "Tenant のすべての職務",
    hint: "職務は、設定した人（依頼者）として実行されます。その人の承認モード・承認ルール・接続が適用されます。職務の仕事は職務専用のスレッドで行われます。",
    empty: "職務はありません。",
    create: "職務を設定",
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
    requester: "依頼者",
    agent: "Agent",
    agentId: "Agent ID",
    agentVersion: "バージョン",
    newer: (version: string) => `新しいバージョン ${version} があります`,
    mission: "使命（毎回の仕事で依頼する内容）",
    procedure: "手順書（毎回の仕事で守るチェックリスト）",
    name: "名前",
    timing: "タイミング",
    cron: "cron 式",
    interval: "一定間隔",
    noTiming: "時刻では起動しない",
    expression: "cron 式（分 時 日 月 曜日）",
    timeZone: "タイムゾーン",
    everyMinutes: "間隔（分）",
    pauseAfterFailures: "何回続けて失敗したら一時停止するか（0 なら止めない）",
    thread: "職務のスレッド",
    mode: "承認モード",
    defaultMode: "自分の既定",
    lateness: "許容する遅れ（分）",
    successor: "後継者のサブジェクト",
    nameSuccessor: "後継者を指名",
    confirmDelete: "この職務を削除しますか？",
    problems: {
      title: "名前を入力してください（200文字以内）",
      mission: "使命を入力してください",
      procedure: "手順書は16,000文字以内です",
      agent: "Agent とバージョンを入力してください",
      expression: "cron 式は5つのフィールドで書いてください",
      time_zone: "タイムゾーンを入力してください",
      interval: "間隔は1分から1年までです",
      lateness: "許容する遅れは1分から7日までです",
      failures: "一時停止までの失敗回数は0から1000までです",
    } satisfies Record<FormProblem, string>,
    status: { active: "有効", paused: "一時停止", stopped: "停止" },
    byAgent: "Agent が登録",
  },
  "en-US": {
    title: "Duties",
    mine: "My Duties",
    tenant: "All Duties of the Tenant",
    hint: "A Duty runs as the person who set it up, its Requester, with their Approval Mode, Approval Rules and connections. Its Tasks work in the Duty's own Thread.",
    empty: "No Duties.",
    create: "Register a Duty",
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
    requester: "Requester",
    agent: "Agent",
    agentId: "Agent ID",
    agentVersion: "Version",
    newer: (version: string) => `Version ${version} is available`,
    mission: "Mission (asked of every Task)",
    procedure: "Procedure (the checklist every Task follows)",
    name: "Name",
    timing: "Timing",
    cron: "Cron expression",
    interval: "Fixed interval",
    noTiming: "No time trigger",
    expression: "Cron expression (minute hour day month weekday)",
    timeZone: "Time zone",
    everyMinutes: "Every (minutes)",
    pauseAfterFailures:
      "Pause after this many failed Tasks in a row (0: never)",
    thread: "The Duty's Thread",
    mode: "Approval Mode",
    defaultMode: "My default",
    lateness: "Maximum lateness (minutes)",
    successor: "Successor subject",
    nameSuccessor: "Name successor",
    confirmDelete: "Delete this Duty?",
    problems: {
      title: "Enter a name of up to 200 characters",
      mission: "Enter the mission",
      procedure: "A procedure has at most 16,000 characters",
      agent: "Enter the Agent and its version",
      expression: "A cron expression has five fields",
      time_zone: "Enter a time zone",
      interval: "The interval is between 1 minute and 1 year",
      lateness: "Lateness is between 1 minute and 7 days",
      failures: "The failures before a pause are 0 to 1000",
    } satisfies Record<FormProblem, string>,
    status: { active: "Active", paused: "Paused", stopped: "Stopped" },
    byAgent: "Registered by an Agent",
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

/** How often a mounted Duty list and history refresh. */
const DUTY_REFRESH_MS = 15_000;

function listUrl(scope: Scope): string {
  if (scope.kind === "workspace")
    return `/api/workspaces/${encodeURIComponent(scope.workspace)}/duties`;
  return `/api/duties?scope=${scope.kind}`;
}

export function DutiesPanel({
  scope,
  administrator = false,
}: {
  scope: Scope;
  /** The viewer holds `duty.administer` and may list the whole Tenant. */
  administrator?: boolean;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [all, setAll] = useState(false);
  const shown: Scope =
    scope.kind === "mine" && all ? { kind: "tenant" } : scope;
  const key = ["duties", shown];
  // The scheduler changes next times, history and stops on its own, so a
  // mounted list follows it as the bell's notices do.
  const list = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<Duty[]>(listUrl(shown)),
    retry: false,
    refetchInterval: DUTY_REFRESH_MS,
  });
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<Duty | "new" | null>(null);
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
      await client.invalidateQueries({ queryKey: ["duties"] });
      await client.invalidateQueries({ queryKey: ["duty"] });
      await client.invalidateQueries({ queryKey: ["duty-notices"] });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Panel title={scope.kind === "workspace" ? words.title : words.mine}>
      <Hint>{words.hint}</Hint>
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
        {list.data?.map((duty) => (
          <li key={duty.id} className="grid gap-2 py-3">
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                className="text-left text-sm font-medium underline-offset-2 hover:underline"
                onClick={() =>
                  setSelected(selected === duty.id ? null : duty.id)
                }
              >
                {duty.title}
              </button>
              <Badge
                variant={duty.status === "active" ? "secondary" : "outline"}
              >
                {words.status[duty.status]}
              </Badge>
              {duty.created_by_run_id && (
                <Badge variant="outline">{words.byAgent}</Badge>
              )}
            </div>
            <div className="text-xs text-muted-foreground">
              {triggerSummary(duty.trigger, ja)} · {words.next}:{" "}
              {formatTime(duty.next_scheduled_at, locale, timeZone)} ·{" "}
              {words.agent}: {duty.agent.id}@{duty.agent.version}
              {!duty.mine && ` · ${words.requester}: ${duty.requester}`}
            </div>
            {duty.halt && duty.halt !== "paused" && (
              <Hint>{haltText(duty.halt, duty.halt_reason, ja)}</Hint>
            )}
            <div className="flex flex-wrap gap-2">
              {dutyActions(duty).edit && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => setEditing(duty)}
                >
                  {words.edit}
                </Button>
              )}
              {dutyActions(duty).pause && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() =>
                    void act(() =>
                      apiFetch(`/api/duties/${duty.id}/pause`, json({})),
                    )
                  }
                >
                  {words.pause}
                </Button>
              )}
              {dutyActions(duty).resume && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() =>
                    void act(() =>
                      apiFetch(`/api/duties/${duty.id}/resume`, json({})),
                    )
                  }
                >
                  {dutyActions(duty).resume === "resume"
                    ? words.resume
                    : words.takeOver}
                </Button>
              )}
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void act(() =>
                    apiFetch(`/api/duties/${duty.id}/run-now`, json({})),
                  )
                }
              >
                {words.runNow}
              </Button>
              {dutyActions(duty).remove && (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => {
                    if (window.confirm(words.confirmDelete))
                      void act(() =>
                        apiFetch(`/api/duties/${duty.id}`, {
                          method: "DELETE",
                        }),
                      );
                  }}
                >
                  {words.remove}
                </Button>
              )}
            </div>
            {selected === duty.id && (
              <DutyHistory
                id={duty.id}
                timeZone={timeZone}
                onSuccessor={(subject) =>
                  void act(() =>
                    apiFetch(
                      `/api/duties/${duty.id}/successor`,
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
        <DutyEditor
          key={editing === "new" ? "new" : editing.id}
          initial={editing === "new" ? emptyForm(timeZone) : formOf(editing)}
          busy={busy}
          onCancel={() => setEditing(null)}
          onSave={(form) =>
            void act(async () => {
              if (editing === "new") {
                if (scope.kind !== "workspace") return;
                await apiFetch(listUrl(scope), json(definitionOf(form)));
              } else {
                await apiFetch(`/api/duties/${editing.id}`, {
                  method: "PATCH",
                  headers: { "content-type": "application/json" },
                  body: JSON.stringify(patchOf(form, editing)),
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

function DutyHistory({
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
    queryKey: ["duty", id],
    queryFn: () => apiFetch<DutyDetail>(`/api/duties/${id}`),
    retry: false,
    refetchInterval: DUTY_REFRESH_MS,
  });
  if (detail.isError) return <Alert>{String(detail.error)}</Alert>;
  if (!detail.data) return <Loading />;
  const { duty, ticks, newer_agent_version, may_administer } = detail.data;
  return (
    <div className="grid gap-2 rounded-md border border-border p-3">
      <p className="whitespace-pre-wrap text-sm">{duty.mission}</p>
      {duty.procedure && (
        <p className="whitespace-pre-wrap text-xs text-faint">
          {duty.procedure}
        </p>
      )}
      {duty.thread_id && (
        <Hint>
          {words.thread}: <span className="font-mono">{duty.thread_id}</span>
        </Hint>
      )}
      {newer_agent_version && duty.mine && (
        <Hint>{words.newer(newer_agent_version)}</Hint>
      )}
      {may_administer && duty.halt === "requester_left" && (
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

function DutyEditor({
  initial,
  busy,
  onCancel,
  onSave,
}: {
  initial: DutyForm;
  busy: boolean;
  onCancel: () => void;
  onSave: (form: DutyForm) => void;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const [form, setForm] = useState(initial);
  const [tried, setTried] = useState(false);
  const problems = formProblems(form);
  const set = <K extends keyof DutyForm>(key: K, value: DutyForm[K]) =>
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
      <Field label={words.mission}>
        <Textarea
          value={form.mission}
          onChange={(event) => set("mission", event.target.value)}
        />
      </Field>
      <Field label={words.procedure}>
        <Textarea
          rows={4}
          value={form.procedure}
          onChange={(event) => set("procedure", event.target.value)}
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
            set("timing", event.target.value as DutyForm["timing"])
          }
        >
          <option value="cron">{words.cron}</option>
          <option value="interval">{words.interval}</option>
          <option value="none">{words.noTiming}</option>
        </NativeSelect>
      </Field>
      {form.timing === "none" ? null : form.timing === "cron" ? (
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
      <Field label={words.pauseAfterFailures}>
        <Input
          inputMode="numeric"
          value={form.pauseAfterFailures}
          onChange={(event) => set("pauseAfterFailures", event.target.value)}
        />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={words.mode}>
          <NativeSelect
            value={form.approvalMode}
            onChange={(event) =>
              set(
                "approvalMode",
                event.target.value as DutyForm["approvalMode"],
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
