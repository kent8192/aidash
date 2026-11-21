/** Duty templates in the Marketplace (#1011 D4): a template's detail, the
 *  dialog that installs one as the person's own Duty, the dialog that
 *  publishes one of their Duties as a template, and the template line on a
 *  Duty's page that offers a later version. */
import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import { authorizationCatalog, registryList } from "./generated/aidash";
import { Field, useI18n } from "./ui";
import { Alert, Hint, Loading } from "./components/patterns";
import { Badge } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./components/ui/dialog";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { triggerSummary, type Duty, type DutyDetail } from "./duties-model";
import {
  agentChoices,
  agentKey,
  defaultAgent,
  installBody,
  lineDiff,
  publishBody,
  publishForm,
  publishProblems,
  slotOf,
  templateOf,
  timeTriggerOf,
  updateBody,
  updateChanges,
  type AgentRef,
  type DutyTemplate,
  type ExpectedTrigger,
  type PublishForm,
  type PublishProblem,
  type TriggerSlot,
  type UpdateChange,
  type UpdateField,
} from "./duty-templates-model";

const copy = {
  "en-US": {
    kind: "Duty template",
    mission: "Mission",
    procedure: "Procedure",
    triggers: "Triggers it expects",
    noTriggers: "No triggers: it runs only when you run it.",
    router: "Router script",
    watcher: "Watcher script",
    suggested: "Suggested Agent",
    noSecrets:
      "A template carries no secrets, trigger secrets or Requester. Installing it makes a Duty that acts as you.",
    slot: {
      time: "Time",
      webhook: "Webhook",
      duty_completed: "Another Duty's Task ends",
      node_event: "Event from a Node",
    } satisfies Record<TriggerSlot, string>,
    install: "Install as a Duty",
    installHelp:
      "The Duty acts as you, set up from the Thread you choose (it opens its own Thread beside it), staffed by the Agent you choose. Its scripts need no approval of their own; shell or Python must be approved for the Node.",
    thread: "Thread",
    agent: "Agent",
    chooseAgent: "Choose an Agent",
    agentId: "Agent ID",
    agentVersion: "Agent version",
    cancel: "Cancel",
    close: "Close",
    installed: "The Duty is set up.",
    createThese:
      "Create these triggers on the Duty's page; a webhook's secret is shown once, when you create it:",
    publish: "Publish as template",
    publishHelp:
      "Shares this Duty's title, mission, procedure, scripts and time trigger as a Marketplace template, without you as Requester, its trigger secrets or ids. It is shared with your Tenant until you share it further.",
    packageId: "Package ID",
    version: "Version",
    author: "Author",
    description: "Description (optional)",
    instruction: (slot: string) => `What to set up: ${slot}`,
    suggestAgent: "Suggest this Duty's Agent",
    published: (key: string) => `Published as ${key}.`,
    problems: {
      package_id: "Enter a package ID without spaces",
      version: "Enter a version such as 1.0.0",
      author: "Enter the author",
      description: "A description has at most 1,000 characters",
      instruction: "Write an instruction for every trigger",
    } satisfies Record<PublishProblem, string>,
    from: (packageId: string, version: string) =>
      `Installed from the template ${packageId} ${version}`,
    update: (version: string) => `Template version ${version} is available`,
    reviewUpdate: "Review the update",
    reviewTitle: (version: string) =>
      `What version ${version} changes in this Duty`,
    noChanges:
      "This version changes neither the mission, the procedure nor the scripts.",
    notTemplate: "This Marketplace version is not a Duty template",
    fields: {
      mission: "Mission",
      procedure: "Procedure",
      router: "Router",
      watcher: "Watcher",
    } satisfies Record<UpdateField, string>,
    applyUpdate: "Apply this version",
  },
  "ja-JP": {
    kind: "職務テンプレート",
    mission: "使命",
    procedure: "手順書",
    triggers: "想定している起動条件",
    noTriggers: "起動条件はありません。手動で実行したときだけ動きます。",
    router: "ルーター",
    watcher: "ウォッチャー",
    suggested: "推奨 Agent",
    noSecrets:
      "テンプレートにはシークレット、トリガーのシークレット、依頼者は含まれません。導入するとあなたを依頼者とする職務になります。",
    slot: {
      time: "時刻",
      webhook: "Webhook",
      duty_completed: "別の職務のタスク完了",
      node_event: "ノードからのイベント",
    } satisfies Record<TriggerSlot, string>,
    install: "職務として導入",
    installHelp:
      "職務はあなたを依頼者として、選んだスレッドから設定され（その隣に職務自身のスレッドを開きます）、選んだ Agent が実行します。スクリプト個別の承認は不要ですが、このノードで shell または Python の承認が必要です。",
    thread: "スレッド",
    agent: "Agent",
    chooseAgent: "Agent を選択",
    agentId: "Agent ID",
    agentVersion: "Agent のバージョン",
    cancel: "キャンセル",
    close: "閉じる",
    installed: "職務を設定しました。",
    createThese:
      "職務のページで次の起動条件を作成してください。Webhook のシークレットは作成時に一度だけ表示されます:",
    publish: "テンプレートとして公開",
    publishHelp:
      "この職務の名前・使命・手順書・スクリプト・時刻の起動条件を Marketplace のテンプレートとして共有します。依頼者（あなた）、起動条件のシークレット、ID は含まれません。さらに共有するまでは自分の Tenant にだけ公開されます。",
    packageId: "パッケージ ID",
    version: "バージョン",
    author: "作成者",
    description: "説明（任意）",
    instruction: (slot: string) => `設定方法: ${slot}`,
    suggestAgent: "この職務の Agent を推奨する",
    published: (key: string) => `${key} として公開しました。`,
    problems: {
      package_id: "空白を含まないパッケージ ID を入力してください",
      version: "1.0.0 のようなバージョンを入力してください",
      author: "作成者を入力してください",
      description: "説明は1,000文字以内です",
      instruction: "すべての起動条件に設定方法を書いてください",
    } satisfies Record<PublishProblem, string>,
    from: (packageId: string, version: string) =>
      `テンプレート ${packageId} ${version} から導入`,
    update: (version: string) =>
      `テンプレートの新しいバージョン ${version} があります`,
    reviewUpdate: "更新内容を確認",
    reviewTitle: (version: string) =>
      `バージョン ${version} でこの職務が変わる点`,
    noChanges: "このバージョンは使命・手順書・スクリプトを変えません。",
    notTemplate:
      "この Marketplace のバージョンは職務テンプレートではありません",
    fields: {
      mission: "使命",
      procedure: "手順書",
      router: "Router",
      watcher: "Watcher",
    } satisfies Record<UpdateField, string>,
    applyUpdate: "このバージョンを適用",
  },
};

const post = (body: unknown): RequestInit => ({
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify(body),
});

function TriggerList({ triggers }: { triggers: ExpectedTrigger[] }) {
  const { locale } = useI18n();
  const words = copy[locale];
  const ja = locale === "ja-JP";
  if (!triggers.length) return <Hint>{words.noTriggers}</Hint>;
  return (
    <ul className="grid gap-1.5 text-xs">
      {triggers.map((trigger) => {
        const slot = slotOf(trigger);
        const time =
          slot === "time"
            ? timeTriggerOf({
                title: "",
                mission: "",
                procedure: "",
                triggers: [trigger],
              })
            : null;
        return (
          <li key={trigger.kind} className="grid gap-0.5">
            <span className="font-medium">
              {words.slot[slot]}
              {time && ` · ${triggerSummary(time, ja)}`}
            </span>
            <span className="whitespace-pre-wrap text-muted-foreground">
              {trigger.instruction}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

/** What a Duty template carries, in the Marketplace's package detail. */
export function DutyTemplateDetail({ template }: { template: DutyTemplate }) {
  const { locale } = useI18n();
  const words = copy[locale];
  return (
    <div className="grid gap-2">
      <Badge variant="secondary" className="justify-self-start">
        {words.kind}
      </Badge>
      <Hint>{words.noSecrets}</Hint>
      <h3 className="text-xs font-semibold">{words.mission}</h3>
      <p className="whitespace-pre-wrap text-sm">{template.mission}</p>
      {template.procedure && (
        <>
          <h3 className="text-xs font-semibold">{words.procedure}</h3>
          <p className="whitespace-pre-wrap text-xs text-muted-foreground">
            {template.procedure}
          </p>
        </>
      )}
      <h3 className="text-xs font-semibold">{words.triggers}</h3>
      <TriggerList triggers={template.triggers} />
      {template.router && (
        <Hint>
          {words.router}: {template.router.language}
        </Hint>
      )}
      {template.watcher && (
        <Hint>
          {words.watcher}: {template.watcher.script.language}
        </Hint>
      )}
      {template.agent && (
        <Hint>
          {words.suggested}:{" "}
          <span className="font-mono">{agentKey(template.agent)}</span>
        </Hint>
      )}
    </div>
  );
}

type ThreadRow = { id: string; title: string };
type InstalledDuty = { duty: Duty; triggers_to_create: ExpectedTrigger[] };

/** The dialog that installs a template as the person's own Duty: they
 *  choose the Thread it is set up from and the Agent, the suggested one by default when
 *  the Node approved it. */
export function DutyTemplateInstall({
  packageKey,
  digest,
  template,
  tenant,
  node,
  open,
  onClose,
}: {
  packageKey: string;
  /** The digest of the version the person reviewed. */
  digest: string;
  template: DutyTemplate;
  tenant: string;
  /** The Node whose Catalog Approvals decide which Agents may be chosen. */
  node?: string;
  open: boolean;
  onClose: () => void;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const client = useQueryClient();
  const threads = useQuery({
    queryKey: ["duty-template", "threads"],
    queryFn: () =>
      apiFetch<{ items: ThreadRow[] }>("/api/threads").then(
        (page) => page.items,
      ),
    enabled: open,
    retry: false,
  });
  const agents = useQuery({
    queryKey: ["duty-template", "agents", tenant, node],
    queryFn: async () => {
      const readable = (await registryList({ kind: "agent" })).map(
        (entry): AgentRef => ({ id: entry.id, version: entry.version }),
      );
      // The approvals narrow the choice when the person may read them.
      const approvals = node
        ? await authorizationCatalog(tenant, { node }).catch(() => undefined)
        : undefined;
      return agentChoices(readable, approvals);
    },
    enabled: open,
    retry: false,
  });
  const suggested = useMemo(
    () => (agents.data ? defaultAgent(template, agents.data) : undefined),
    [agents.data, template],
  );
  const [thread, setThread] = useState("");
  const [chosen, setChosen] = useState<string>();
  const [manual, setManual] = useState<AgentRef>({ id: "", version: "" });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [installed, setInstalled] = useState<InstalledDuty>();
  // One key per opened dialog, so a retried click returns the same Duty.
  const [requestKey] = useState(() => crypto.randomUUID());
  const choice = chosen ?? (suggested ? agentKey(suggested) : "");
  const agent: AgentRef | undefined = agents.data?.length
    ? agents.data.find((value) => agentKey(value) === choice)
    : manual.id.trim() && manual.version.trim()
      ? { id: manual.id.trim(), version: manual.version.trim() }
      : undefined;
  const target = thread || threads.data?.[0]?.id || "";
  async function install() {
    if (busy || !target) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<InstalledDuty>(
        `/api/marketplace/packages/${encodeURIComponent(packageKey)}/duty`,
        post(installBody(digest, target, agent, requestKey)),
      );
      setInstalled(result);
      await client.invalidateQueries({ queryKey: ["duties"] });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !busy) onClose();
      }}
    >
      <DialogContent className="max-w-xl" closeLabel={words.close}>
        <DialogHeader>
          <DialogTitle>{words.install}</DialogTitle>
          <DialogDescription className="text-xs">
            {words.installHelp}
          </DialogDescription>
        </DialogHeader>
        {installed ? (
          <div className="grid gap-2">
            <Hint>
              {words.installed} {installed.duty.title}
            </Hint>
            {installed.triggers_to_create.length > 0 && (
              <>
                <Hint>{words.createThese}</Hint>
                <TriggerList triggers={installed.triggers_to_create} />
              </>
            )}
          </div>
        ) : (
          <div className="grid gap-3">
            {(threads.isLoading || agents.isLoading) && <Loading />}
            {threads.isError && <Alert>{String(threads.error)}</Alert>}
            <Field label={words.thread}>
              <NativeSelect
                value={target}
                onChange={(event) => setThread(event.target.value)}
              >
                {(threads.data ?? []).map((row) => (
                  <option key={row.id} value={row.id}>
                    {row.title}
                  </option>
                ))}
              </NativeSelect>
            </Field>
            {agents.data?.length ? (
              <Field label={words.agent}>
                <NativeSelect
                  value={choice}
                  onChange={(event) => setChosen(event.target.value)}
                >
                  <option value="">{words.chooseAgent}</option>
                  {agents.data.map((value) => (
                    <option key={agentKey(value)} value={agentKey(value)}>
                      {agentKey(value)}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
            ) : (
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label={words.agentId}>
                  <Input
                    value={manual.id}
                    onChange={(event) =>
                      setManual({ ...manual, id: event.target.value })
                    }
                  />
                </Field>
                <Field label={words.agentVersion}>
                  <Input
                    value={manual.version}
                    onChange={(event) =>
                      setManual({ ...manual, version: event.target.value })
                    }
                  />
                </Field>
              </div>
            )}
            {error && <Alert>{error}</Alert>}
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" disabled={busy} onClick={onClose}>
            {installed ? words.close : words.cancel}
          </Button>
          {!installed && (
            <Button
              disabled={busy || !target || !agent}
              onClick={() => void install()}
            >
              {words.install}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** "Publish as template" on one of the person's own Duties. */
export function PublishDutyTemplate({ duty }: { duty: Duty }) {
  const { locale } = useI18n();
  const words = copy[locale];
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const draft = useQuery({
    queryKey: ["duty-template-draft", duty.id],
    queryFn: () => apiFetch<DutyTemplate>(`/api/duties/${duty.id}/template`),
    enabled: open,
    retry: false,
  });
  const [form, setForm] = useState<PublishForm>();
  const [tried, setTried] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState("");
  const [requestKey, setRequestKey] = useState(() => crypto.randomUUID());
  const current =
    form ?? (draft.data ? publishForm(draft.data, duty.template) : undefined);
  const problems =
    current && draft.data ? publishProblems(current, draft.data) : [];
  const set = <K extends keyof PublishForm>(key: K, value: PublishForm[K]) =>
    setForm({ ...current!, [key]: value });
  async function publish() {
    setTried(true);
    if (busy || !current || !draft.data || problems.length) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<{ key: string }>(
        `/api/duties/${duty.id}/template`,
        post(publishBody(current, draft.data, requestKey)),
      );
      setDone(words.published(result.key));
      await client.invalidateQueries({ queryKey: ["marketplace"] });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  function close() {
    if (busy) return;
    setOpen(false);
    setForm(undefined);
    setTried(false);
    setError("");
    setDone("");
    setRequestKey(crypto.randomUUID());
  }
  return (
    <>
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
        {words.publish}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!next) close();
        }}
      >
        <DialogContent className="max-w-xl" closeLabel={words.close}>
          <DialogHeader>
            <DialogTitle>{words.publish}</DialogTitle>
            <DialogDescription className="text-xs">
              {words.publishHelp}
            </DialogDescription>
          </DialogHeader>
          {draft.isError && <Alert>{String(draft.error)}</Alert>}
          {draft.isLoading && <Loading />}
          {done ? (
            <Hint>{done}</Hint>
          ) : (
            current &&
            draft.data && (
              <div className="grid gap-3">
                <div className="grid gap-3 sm:grid-cols-2">
                  <Field label={words.packageId}>
                    <Input
                      value={current.packageId}
                      onChange={(event) => set("packageId", event.target.value)}
                    />
                  </Field>
                  <Field label={words.version}>
                    <Input
                      value={current.version}
                      onChange={(event) => set("version", event.target.value)}
                    />
                  </Field>
                </div>
                <Field label={words.author}>
                  <Input
                    value={current.author}
                    onChange={(event) => set("author", event.target.value)}
                  />
                </Field>
                <Field label={words.description}>
                  <Textarea
                    rows={2}
                    value={current.description}
                    onChange={(event) => set("description", event.target.value)}
                  />
                </Field>
                {draft.data.triggers.map((trigger) => {
                  const slot = slotOf(trigger);
                  return (
                    <Field
                      key={slot}
                      label={words.instruction(words.slot[slot])}
                    >
                      <Textarea
                        rows={2}
                        value={current.instructions[slot] ?? ""}
                        onChange={(event) =>
                          set("instructions", {
                            ...current.instructions,
                            [slot]: event.target.value,
                          })
                        }
                      />
                    </Field>
                  );
                })}
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={current.suggestAgent}
                    onChange={(event) =>
                      set("suggestAgent", event.target.checked)
                    }
                  />
                  {words.suggestAgent} ({duty.agent.id}@{duty.agent.version})
                </label>
                {tried &&
                  problems.map((problem) => (
                    <Alert key={problem}>{words.problems[problem]}</Alert>
                  ))}
                {error && <Alert>{error}</Alert>}
              </div>
            )
          )}
          <DialogFooter>
            <Button variant="ghost" disabled={busy} onClick={close}>
              {done ? words.close : words.cancel}
            </Button>
            {!done && (
              <Button
                disabled={busy || !current}
                onClick={() => void publish()}
              >
                {words.publish}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

/** The template a Duty came from, and a later version its Requester may
 *  apply. A later version is offered, never applied by itself, and never
 *  unseen: the person first reviews what it changes, then applies exactly
 *  the version they reviewed. */
export function DutyTemplateLine({
  detail,
  onUpdated,
}: {
  detail: DutyDetail;
  onUpdated: () => void;
}) {
  const { locale } = useI18n();
  const words = copy[locale];
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [review, setReview] = useState<{
    digest: string;
    changes: UpdateChange[];
  }>();
  const { duty, template_update: update } = detail;
  if (!duty.template) return null;
  async function load() {
    if (busy || !update) return;
    setBusy(true);
    setError("");
    try {
      const shown = await apiFetch<{
        summary: { digest: string };
        manifest: { entity: { kind: string; config?: unknown } };
      }>(`/api/marketplace/packages/${encodeURIComponent(update.key)}`);
      const template = templateOf(shown.manifest);
      if (!template) throw new Error(words.notTemplate);
      setReview({
        digest: shown.summary.digest,
        changes: updateChanges(duty, template),
      });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  async function apply() {
    if (busy || !update || !review) return;
    setBusy(true);
    setError("");
    try {
      await apiFetch(
        `/api/duties/${duty.id}/template-update`,
        post(updateBody(update.key, review.digest)),
      );
      setReview(undefined);
      onUpdated();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="grid gap-1">
      <Hint>{words.from(duty.template.package_id, duty.template.version)}</Hint>
      {update && detail.may_edit && !review && (
        <div className="flex flex-wrap items-center gap-2">
          <Hint>{words.update(update.version)}</Hint>
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => void load()}
          >
            {words.reviewUpdate}
          </Button>
        </div>
      )}
      {update && review && (
        <section
          aria-label={words.reviewTitle(update.version)}
          className="grid gap-2 rounded-md border p-2"
        >
          <Hint>{words.reviewTitle(update.version)}</Hint>
          {review.changes.length === 0 && <Hint>{words.noChanges}</Hint>}
          {review.changes.map((change) => (
            <div key={change.field} className="grid gap-1">
              <span className="text-xs font-medium">
                {words.fields[change.field]}
              </span>
              <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">
                {lineDiff(change.before, change.after).map((line, index) => (
                  <div
                    key={index}
                    className={
                      line.kind === "+"
                        ? "text-green-700 dark:text-green-400"
                        : line.kind === "-"
                          ? "text-red-700 dark:text-red-400"
                          : undefined
                    }
                  >
                    {`${line.kind} ${line.text}`}
                  </div>
                ))}
              </pre>
            </div>
          ))}
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={busy} onClick={() => void apply()}>
              {words.applyUpdate}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => setReview(undefined)}
            >
              {words.cancel}
            </Button>
          </div>
        </section>
      )}
      {error && <Alert>{error}</Alert>}
    </div>
  );
}
