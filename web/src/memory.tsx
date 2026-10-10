import { nativeMemoryProvider } from "./agent-bindings";
import { useState } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { State, EntityRef } from "./types";
import { apiFetch, ApiError } from "./transport";
import { History as HistoryIcon, Plus, RefreshCw, Search } from "lucide-react";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Badge, Empty, Field, Panel, Modal, useI18n } from "./ui";
import {
  Alert,
  Check,
  Disclosure,
  Group,
  Hint,
  Notice,
} from "./components/patterns";
import {
  MemoryContent,
  MemoryOverview,
  MemoryRecord,
  unitAnchor,
} from "./memory-view";

type Bank = {
  home: string;
  tenant: string;
  workspace: string;
  participant: string | null;
};
export type Evidence = {
  kind: string;
  id: string;
  revision: number;
  bank?: Bank;
  digest?: string;
};
export type Content = {
  mental_model?: { question: string; automatic_refresh: boolean } | null;
  text: string;
  kind: "world" | "experience" | "observation" | "mental_model";
  learning: "fact" | "preference" | "procedure" | "failure";
  verification: "unverified" | "supported" | "contradicted";
  occurred: { start: string; end: string } | null;
  entities: { name: string; category: string; aliases?: string[] }[];
  evidence: Evidence[];
  links: { target: string; revision: number; kind: string; weight: number }[];
};
export type Unit = {
  id: string;
  bank: Bank;
  revision: number;
  content: Content;
  learned_at: string;
  updated_at: string;
  deleted: boolean;
  stale: boolean;
};
type Participant = {
  id: string;
  bank: Bank;
  agent: EntityRef;
  revision: number;
};
type Candidate = {
  id: string;
  revision: number;
  run: Evidence;
  content: Content;
  state: string;
};
type Request = {
  operation_id: string;
  provider: EntityRef;
  bank: Bank;
  action: object;
};
type Outcome = { result: string; value: unknown };
type Inspection = {
  items: unknown[];
  next: string | null;
  purge_jobs: unknown[];
  model_operations: number;
  pending_candidates: number;
};
type HistoryRecord = { revision: number; [key: string]: unknown };
type Settings = { provider: EntityRef; revision: number };
type Assigned = { participant_id: string; participant_revision: number };
const post = <T,>(url: string, value: unknown) =>
  apiFetch<T>(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(value),
  });
const subheading = "text-xs font-semibold text-foreground";
const refKey = (ref: EntityRef) => `${ref.id}@${ref.version}`;

/** Bodies live in Workspace memory. Registry selection contributes immutable references only. */
export function MemoryWorkspace({
  data,
  workspace,
}: {
  data: State;
  workspace: string;
}) {
  const { locale, local } = useI18n();
  const ja = locale === "ja-JP";
  const text = (en: string, jp: string) => (ja ? jp : en);
  const client = useQueryClient();
  const [tenant, setTenant] = useState(
    data.access.kind === "subject" ? data.access.tenant : "",
  );
  const [participantId, setParticipantId] = useState("");
  const [providerKey, setProviderKey] = useState("");
  const [agentKey, setAgentKey] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState<{ url: string; body: unknown } | null>(
    null,
  );
  const [editing, setEditing] = useState<Unit | "new" | null>(null);
  const [reviewing, setReviewing] = useState<Candidate | null>(null);
  const [conflict, setConflict] = useState(false);
  const [result, setResult] = useState<Outcome | null>(null);
  const [historyUnit, setHistoryUnit] = useState<Unit | null>(null);
  const [observed, setObserved] = useState({ scope: "", units: "" });
  const [query, setQuery] = useState("");
  const [taskId, setTaskId] = useState("");
  const [questionSource, setQuestionSource] = useState<Unit | null>(null);
  const [question, setQuestion] = useState("");
  const [questionTarget, setQuestionTarget] = useState<Unit | null>(null);
  const [questionSources, setQuestionSources] = useState<string[]>([]);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [publicationProviderKey, setPublicationProviderKey] = useState("");
  const base = `/api/workspaces/${workspace}/memory`;
  const providers = data.registry.filter((entry) => entry.kind === "memory");
  const agents = data.registry.filter((entry) => entry.kind === "agent");
  const participants = useInfiniteQuery({
    queryKey: ["memory", workspace, "participants"],
    initialPageParam: "",
    queryFn: ({ pageParam }) =>
      apiFetch<{ items: Participant[]; next: string | null }>(
        `${base}/participants${pageParam ? `?after=${encodeURIComponent(pageParam)}` : ""}`,
      ),
    getNextPageParam: (page) => page.next || undefined,
    retry: false,
  });
  const allParticipants =
    participants.data?.pages.flatMap((page) => page.items) ?? [];
  const chosen = allParticipants.find(
    (participant) => participant.id === participantId,
  );
  const agent = agents.find(
    (entry) =>
      chosen &&
      entry.id === chosen.agent.id &&
      entry.version === chosen.agent.version,
  );
  const boundProvider = nativeMemoryProvider(agent?.config, data.registry);
  const provider = chosen
    ? boundProvider
    : providers.find((entry) => refKey(entry) === providerKey);
  const providerRef = provider
    ? { id: provider.id, version: provider.version }
    : undefined;
  const providerDefinition = providers.find(
    (entry) => providerRef && refKey(entry) === refKey(providerRef),
  );
  const contextBound = (
    providerDefinition?.config.policy as
      | { bounds?: { max_context_tokens?: number } }
      | undefined
  )?.bounds?.max_context_tokens;
  const contextTokens =
    typeof contextBound === "number" &&
    Number.isSafeInteger(contextBound) &&
    contextBound > 0
      ? Math.min(4096, contextBound)
      : 0;
  const bank: Bank = chosen?.bank ?? {
    home: data.node.id,
    tenant,
    workspace,
    participant: null,
  };
  const read =
    providerRef && bank.tenant ? { provider: providerRef, bank } : undefined;
  const assignment = useQuery({
    queryKey: ["memory", workspace, "assignment", taskId],
    queryFn: () =>
      apiFetch<{ task_revision: number; participant: Assigned | null }>(
        `/api/workspaces/${workspace}/tasks/${taskId}/memory-participant`,
      ),
    enabled: !!taskId && !!chosen,
    retry: false,
  });
  const sharedBank = { ...bank, participant: null };
  const sharedSettings = useQuery({
    queryKey: ["memory", workspace, "shared-settings", sharedBank, providerRef],
    queryFn: () =>
      post<{ result: "settings"; value: Settings | null }>(`${base}/operate`, {
        provider: providerRef,
        bank: sharedBank,
        operation_id: crypto.randomUUID(),
        action: { action: "settings" },
      }),
    enabled: !!chosen && !!read,
    retry: false,
  });
  const publicationProvider =
    sharedSettings.data?.value?.provider ??
    providers.find((entry) => refKey(entry) === publicationProviderKey);
  const blocked = busy || retry !== null;
  const units = useQuery({
    queryKey: ["memory", workspace, "units", read],
    queryFn: () => post<Unit[]>(`${base}/units/query`, read),
    enabled: !!read,
    retry: false,
    refetchInterval: 5000,
  });
  const settings = useQuery({
    queryKey: ["memory", workspace, "settings", read],
    queryFn: () =>
      post<{ result: "settings"; value: Settings | null }>(`${base}/operate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        action: { action: "settings" },
      }),
    enabled: !!read,
    retry: false,
  });
  const candidates = useQuery({
    queryKey: ["memory", workspace, "candidates", read],
    queryFn: () =>
      post<{ result: "candidates"; value: Candidate[] }>(`${base}/operate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        action: { action: "candidates" },
      }),
    enabled: !!read && !!chosen,
    retry: false,
    refetchInterval: 5000,
  });
  const inspection = useInfiniteQuery({
    initialPageParam: "",
    queryKey: ["memory", workspace, "inspection", read],
    queryFn: ({ pageParam }) =>
      post<{ result: "inspection"; value: Inspection }>(`${base}/operate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        action: { action: "inspect", after: pageParam || null },
      }),
    getNextPageParam: (page) => page.value.next || undefined,
    enabled: !!read,
    retry: false,
    refetchInterval: 10000,
  });
  const jobs = useInfiniteQuery({
    queryKey: ["memory", workspace, "jobs", read],
    initialPageParam: "",
    queryFn: ({ pageParam }) =>
      post<{
        result: "jobs";
        value: { items: unknown[]; next: string | null };
      }>(`${base}/operate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        action: { action: "jobs", after: pageParam || null },
      }),
    getNextPageParam: (page) => page.value.next || undefined,
    enabled: !!read,
    retry: false,
    refetchInterval: 10000,
  });
  const scope = JSON.stringify([
    workspace,
    tenant,
    participantId,
    providerKey,
    read,
  ]);
  const unitStamp = JSON.stringify(
    units.data?.map((unit) => [unit.id, unit.revision]) ?? null,
  );
  // Reset snapshots before committing a changed scope/revision render. An
  // effect would first paint the old result, then trigger another render.
  if (observed.scope !== scope || observed.units !== unitStamp) {
    setObserved({ scope, units: unitStamp });
    setResult(null);
    if (observed.scope !== scope) setHistoryUnit(null);
  }
  const historyCurrent =
    !!historyUnit &&
    observed.scope === scope &&
    !units.error &&
    (units.data ?? []).some(
      (unit) =>
        unit.id === historyUnit.id && unit.revision === historyUnit.revision,
    );
  const history = useInfiniteQuery({
    queryKey: [
      "memory",
      workspace,
      "history",
      read,
      historyUnit?.id,
      historyUnit?.revision,
    ],
    initialPageParam: 0,
    queryFn: ({ pageParam }) =>
      post<{ result: "history"; value: HistoryRecord[] }>(`${base}/operate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        action: {
          action: "history",
          id: historyUnit!.id,
          expected_revision: historyUnit!.revision,
          before: pageParam || null,
        },
      }),
    getNextPageParam: (page) =>
      page.value.length === 32 && page.value.at(-1)!.revision > 1
        ? page.value.at(-1)!.revision
        : undefined,
    enabled: !!read && historyCurrent,
    retry: false,
    refetchInterval: 10000,
  });
  async function send(url: string, body: unknown) {
    if (busy || (retry && (url !== retry.url || body !== retry.body))) return;
    setBusy(true);
    setResult(null);
    setError("");
    setConflict(false);
    setRetry({ url, body });
    try {
      const response = await post<Outcome>(url, body);
      setResult(response);
      setRetry(null);
      setEditing(null);
      setReviewing(null);
      setQuestionSource(null);
      await client.invalidateQueries({ queryKey: ["memory", workspace] });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      if (e instanceof ApiError && e.status === 409) {
        setConflict(true);
        setRetry(null);
      }
    } finally {
      setBusy(false);
    }
  }
  function operate(action: object) {
    if (read)
      void send(`${base}/operate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        action,
      } satisfies Request);
  }
  function change(operation: object) {
    if (read)
      void send(`${base}/units/mutate`, {
        ...read,
        operation_id: crypto.randomUUID(),
        changes: [operation],
      });
  }
  const failures = [
    participants.error,
    units.error,
    settings.error,
    candidates.error,
    assignment.error,
    sharedSettings.error,
  ].filter(Boolean);
  const modalNotice = (error || conflict) && (
    <Alert>
      <div className="grid gap-2">
      <p>{error}</p>
      {conflict && (
        <p>
          {text(
            "The observed revision changed. Close this editor, refresh and review the current record.",
            "確認したrevisionが変わりました。編集画面を閉じて再読み込みし、現在の内容を確認してください。",
          )}
        </p>
      )}
      {retry && (
        <div>
          <Button
            variant="outline"
            size="sm"
            type="button"
            disabled={busy}
            onClick={() => void send(retry.url, retry.body)}
          >
            {text("Retry the same operation", "同じ操作を再試行")}
          </Button>
        </div>
      )}
      </div>
    </Alert>
  );
  const visibleUnits = units.error ? [] : (units.data ?? []);
  return (
    <Panel
      title={text("Hindsight memory", "Hindsight メモリ")}
      action={
        <Button
          variant="ghost"
          size="sm"
          type="button"
          disabled={blocked}
          onClick={() => {
            setEditing(null);
            setConflict(false);
            void client.invalidateQueries({ queryKey: ["memory", workspace] });
          }}
        >
          <RefreshCw aria-hidden />
          {text("Refresh", "再読み込み")}
        </Button>
      }
    >
      <Hint>
        {text(
          "Private memory belongs to a logical Agent in this Workspace. Publish only selected units to share them.",
          "プライベート記憶はこのWorkspaceの論理Agentに属します。選択したUnitを公開すると共有記憶になります。",
        )}
      </Hint>
      <div className="grid items-end gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {data.access.kind === "operator" && (
          <Field label={text("Tenant", "テナント")}>
            <Input
              className="font-mono"
              disabled={blocked}
              value={tenant}
              onChange={(e) => setTenant(e.target.value)}
            />
          </Field>
        )}
        <Field label={text("Memory bank", "記憶の所有者")}>
          <NativeSelect
            value={participantId}
            disabled={blocked}
            onChange={(e) => {
              setParticipantId(e.target.value);
              setResult(null);
              setEditing(null);
            }}
          >
            <option value="">
              {text("Shared Workspace memory", "Workspace共有記憶")}
            </option>
            {allParticipants.map((value) => (
              <option key={value.id} value={value.id}>
                {refKey(value.agent)} · {value.id.slice(0, 8)} · r
                {value.revision}
              </option>
            ))}
          </NativeSelect>
        </Field>
        {!chosen && (
          <Field
            label={text(
              "Memory provider version",
              "メモリプロバイダのバージョン",
            )}
          >
            <NativeSelect
              value={providerKey}
              disabled={blocked}
              onChange={(e) => setProviderKey(e.target.value)}
            >
              <option value="">
                {text(
                  "Choose a registered provider",
                  "登録済みプロバイダを選択",
                )}
              </option>
              {providers.map((value) => (
                <option key={refKey(value)} value={refKey(value)}>
                  {local(value.name)} · {refKey(value)}
                </option>
              ))}
            </NativeSelect>
          </Field>
        )}
        {chosen && (
          <p className="grid gap-0.5 text-xs">
            <span className="text-muted-foreground">
              {text(
                "Accepted Agent and provider",
                "受け入れ済みAgentとプロバイダ",
              )}
            </span>
            <span className="truncate font-mono text-foreground">
              {refKey(chosen.agent)} ·{" "}
              {providerRef
                ? refKey(providerRef)
                : text("Memory disabled", "メモリ無効")}
            </span>
          </p>
        )}
      </div>
      {participants.hasNextPage && (
        <div>
          <Button
            variant="ghost"
            size="sm"
            type="button"
            disabled={participants.isFetchingNextPage}
            onClick={() => void participants.fetchNextPage()}
          >
            {text("More Agents", "Agentをさらに表示")}
          </Button>
        </div>
      )}
      <div className="flex flex-wrap items-end gap-2">
        <div className="w-full min-w-0 sm:w-72">
          <Field label={text("Agent version", "Agentのバージョン")}>
            <NativeSelect
              value={agentKey}
              disabled={blocked}
              onChange={(e) => setAgentKey(e.target.value)}
            >
              <option value="">{text("Choose an Agent", "Agentを選択")}</option>
              {agents.map((value) => (
                <option key={refKey(value)} value={refKey(value)}>
                  {local(value.name)} · {refKey(value)}
                </option>
              ))}
            </NativeSelect>
          </Field>
        </div>
        <Button
          variant="outline"
          type="button"
          disabled={blocked || !agentKey}
          onClick={() => {
            const value = agents.find((entry) => refKey(entry) === agentKey);
            if (value)
              void send(`${base}/participants`, {
                agent: { id: value.id, version: value.version },
              });
          }}
        >
          {text("Create empty logical Agent", "空の論理Agentを作成")}
        </Button>
        {chosen && (
          <Button
            variant="outline"
            type="button"
            disabled={blocked || !agentKey || refKey(chosen.agent) === agentKey}
            onClick={() => {
              const value = agents.find((entry) => refKey(entry) === agentKey);
              if (value)
                void send(`${base}/participants/${chosen.id}/upgrade`, {
                  agent: { id: value.id, version: value.version },
                  expected_revision: chosen.revision,
                });
            }}
          >
            {text(
              "Upgrade this Agent; preserve its memory",
              "このAgentを更新し、記憶を継続",
            )}
          </Button>
        )}
      </div>
      {chosen && (
        <div className="flex flex-wrap items-end gap-2">
          <div className="w-full min-w-0 sm:w-72">
            <Field label={text("Task", "タスク")}>
              <NativeSelect
                disabled={blocked}
                value={taskId}
                onChange={(e) => setTaskId(e.target.value)}
              >
                <option value="">{text("Choose a Task", "Taskを選択")}</option>
                {data.tasks
                  .filter((task) => task.workspace_id === workspace)
                  .map((task) => (
                    <option key={task.id} value={task.id}>
                      {task.title}
                    </option>
                  ))}
              </NativeSelect>
            </Field>
          </div>
          <Button
            variant="outline"
            type="button"
            disabled={blocked || !taskId || !assignment.data}
            onClick={() => {
              if (assignment.data)
                void send(
                  `/api/workspaces/${workspace}/tasks/${taskId}/memory-participant`,
                  {
                    task_revision: assignment.data.task_revision,
                    expected: assignment.data.participant,
                    target: {
                      participant_id: chosen.id,
                      participant_revision: chosen.revision,
                    },
                  },
                );
            }}
          >
            {text("Assign this Agent to the Task", "このAgentをTaskに割り当て")}
          </Button>
          {assignment.data?.participant && (
            <>
              <p className="self-center font-mono text-xs text-muted-foreground">
                {text("Current assignment", "現在の割当")}:{" "}
                {assignment.data.participant.participant_id.slice(0, 8)} · r
                {assignment.data.participant.participant_revision}
              </p>
              <Button
                variant="ghost"
                type="button"
                disabled={blocked}
                onClick={() =>
                  void send(
                    `/api/workspaces/${workspace}/tasks/${taskId}/memory-participant`,
                    {
                      task_revision: assignment.data!.task_revision,
                      expected: assignment.data!.participant,
                      target: null,
                    },
                  )
                }
              >
                {text("Remove this assignment", "この割当を解除")}
              </Button>
            </>
          )}
        </div>
      )}
      {!!read && !chosen && (
        <div>
          <Button
            variant="outline"
            type="button"
            disabled={blocked || settings.isLoading}
            onClick={() =>
              operate({
                action: "configure_bank",
                expected_revision: settings.data?.value?.revision ?? 0,
              })
            }
          >
            {text(
              "Accept this shared memory policy",
              "この共有メモリポリシーを受け入れ",
            )}
          </Button>
        </div>
      )}
      {(error || failures.length > 0) && (
        <Alert>
          <div className="grid gap-2">
          <p>{error || failures.map((value) => String(value)).join(" · ")}</p>
          {conflict && (
            <p>
              {text(
                "The observed revision changed. Refresh and review the new state before editing again.",
                "確認したrevisionが変わりました。再読み込みして内容を確認し、改めて編集してください。",
              )}
            </p>
          )}
          {retry && (
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                type="button"
                disabled={busy}
                onClick={() => void send(retry.url, retry.body)}
              >
                {text("Retry the same operation", "同じ操作を再試行")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                disabled={busy}
                onClick={() => {
                  setRetry(null);
                  setEditing(null);
                  void client.invalidateQueries({
                    queryKey: ["memory", workspace],
                  });
                }}
              >
                {text(
                  "Dismiss retry and refresh recorded state",
                  "再試行を取り消して保存状態を確認",
                )}
              </Button>
            </div>
          )}
          </div>
        </Alert>
      )}
      {!!read && (
        <div className="grid gap-3 border-t border-border pt-4">
          <div className="flex flex-wrap items-end gap-2">
            {chosen && (
              <div className="w-full min-w-0 sm:w-72">
                <Field
                  label={text(
                    "Shared publication policy",
                    "公開先の共有メモリポリシー",
                  )}
                >
                  <NativeSelect
                    value={
                      publicationProvider ? refKey(publicationProvider) : ""
                    }
                    disabled={blocked || !!sharedSettings.data?.value}
                    onChange={(event) =>
                      setPublicationProviderKey(event.target.value)
                    }
                  >
                    <option value="">
                      {text("Choose the shared policy", "共有ポリシーを選択")}
                    </option>
                    {providers.map((value) => (
                      <option key={refKey(value)} value={refKey(value)}>
                        {local(value.name)} · {refKey(value)}
                      </option>
                    ))}
                  </NativeSelect>
                </Field>
              </div>
            )}
            <Button
              variant="outline"
              type="button"
              disabled={blocked}
              onClick={() => setEditing("new")}
            >
              <Plus aria-hidden />
              {text("Add a memory unit", "Unitを追加")}
            </Button>
            <Button
              variant="ghost"
              type="button"
              disabled={blocked}
              onClick={() => operate({ action: "usage", after: null })}
            >
              {text("Model usage and charged cost", "モデル使用量・計上コスト")}
            </Button>
          </div>
          <form
            className="flex flex-wrap items-end gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (!contextTokens) return;
              operate({
                action: "recall",
                query: {
                  text: query,
                  time: null,
                  kinds: [],
                  max_tokens: contextTokens,
                },
              });
            }}
          >
            <div className="min-w-0 flex-1 basis-64">
              <Field label={text("Recall or reflect", "記憶の検索・考察")}>
                <Input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  required
                />
              </Field>
            </div>
            <Button disabled={blocked || !contextTokens}>
              <Search aria-hidden />
              {text("Recall", "検索")}
            </Button>
            <Button
              variant="outline"
              type="button"
              disabled={blocked || !contextTokens || !query.trim()}
              onClick={() =>
                operate({
                  action: "reflect",
                  query: {
                    text: query,
                    time: null,
                    kinds: [],
                    max_tokens: contextTokens,
                  },
                })
              }
            >
              {text("Reflect with cited evidence", "根拠付きで考察")}
            </Button>
            {!contextTokens && (
              <Notice tone="warning" role="status" className="basis-full">
                {text(
                  "This memory policy is unavailable.",
                  "この記憶ポリシーは利用できません。",
                )}
              </Notice>
            )}
          </form>
        </div>
      )}
      {result && failures.length === 0 && !inspection.error && (
        <Disclosure
          open
          className="rounded-lg border border-brand-line bg-surface px-3 py-2"
          summary={text("Operation result", "操作結果")}
        >
          <MemoryRecord value={result} />
        </Disclosure>
      )}
      {units.data?.length === 0 && <Empty />}
      {!units.error && units.data && <MemoryOverview units={units.data} />}
      <div className="grid gap-3">
        {!units.error &&
          units.data?.map((unit) => (
            <article
              className="grid min-w-0 gap-3 rounded-lg border border-border bg-surface p-4 outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring focus:border-brand-line"
              id={unitAnchor(unit.id)}
              tabIndex={-1}
              key={unit.id}
            >
              <MemoryContent
                content={unit.content}
                unit={unit}
                units={visibleUnits}
              />
              <div className="flex flex-wrap gap-1.5 border-t border-border pt-3">
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  disabled={blocked}
                  onClick={() => setHistoryUnit(unit)}
                >
                  <HistoryIcon aria-hidden />
                  {text("History", "履歴")}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  disabled={blocked}
                  onClick={() => {
                    setConflict(false);
                    setEditing(unit);
                  }}
                >
                  {text("Correct", "訂正")}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  disabled={blocked}
                  onClick={() => {
                    setQuestionSource(unit);
                    setQuestionTarget(null);
                    setQuestionSources([unit.id]);
                    setQuestion("");
                  }}
                >
                  {text("Create a recurring question", "定期的な問いを作成")}
                </Button>
                {unit.content.mental_model && (
                  <Button
                    variant="ghost"
                    size="sm"
                    type="button"
                    disabled={blocked}
                    onClick={() => {
                      setQuestionSource(unit);
                      setQuestionTarget(unit);
                      setQuestion(unit.content.mental_model!.question);
                      setAutoRefresh(
                        unit.content.mental_model!.automatic_refresh,
                      );
                      setQuestionSources(
                        unit.content.evidence
                          .filter((proof) => proof.kind === "unit")
                          .map((proof) => proof.id),
                      );
                    }}
                  >
                    {text(
                      "Refresh this answer with selected evidence",
                      "根拠を選択して回答を更新",
                    )}
                  </Button>
                )}
                {chosen && (
                  <Button
                    variant="ghost"
                    size="sm"
                    type="button"
                    disabled={
                      blocked ||
                      !publicationProvider ||
                      sharedSettings.isLoading
                    }
                    onClick={() => {
                      const operation = crypto.randomUUID();
                      const shared = { ...bank, participant: null };
                      void send(`${base}/operate`, {
                        operation_id: operation,
                        provider: publicationProvider && {
                          id: publicationProvider.id,
                          version: publicationProvider.version,
                        },
                        bank: shared,
                        action: {
                          action: "publish",
                          source: {
                            kind: "unit",
                            bank,
                            id: unit.id,
                            revision: unit.revision,
                          },
                          mutation: {
                            operation_id: operation,
                            provider: publicationProvider && {
                              id: publicationProvider.id,
                              version: publicationProvider.version,
                            },
                            bank: shared,
                            changes: [
                              {
                                operation: "add",
                                id: crypto.randomUUID(),
                                content: unit.content,
                              },
                            ],
                          },
                        },
                      });
                    }}
                  >
                    {text(
                      "Publish selected unit to Workspace",
                      "このUnitをWorkspaceに公開",
                    )}
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  className="ml-auto hover:text-destructive"
                  disabled={blocked}
                  onClick={() =>
                    change({
                      operation: "delete",
                      id: unit.id,
                      expected_revision: unit.revision,
                    })
                  }
                >
                  {text("Delete this revision", "このrevisionを削除")}
                </Button>
              </div>
            </article>
          ))}
      </div>
      {candidates.data && !candidates.error && (
        <Panel
          title={text("Run learning review", "Runからの学習候補のレビュー")}
        >
          <Hint>
            {text(
              "Run completion does not verify a claim. Admission is a human decision.",
              "Runの完了は主張の正しさを証明しません。人が内容を確認して採用します。",
            )}
          </Hint>
          {candidates.data.value.map((candidate) => (
            <article
              key={candidate.id}
              className="grid min-w-0 gap-3 rounded-lg border border-border bg-surface p-4"
            >
              <div>
                <Badge value={candidate.state} />
              </div>
              <MemoryContent content={candidate.content} units={visibleUnits} />
              <MemoryRecord
                value={{
                  source: candidate.run,
                  evidence: candidate.content.evidence,
                }}
              />
              <div className="flex flex-wrap gap-2 border-t border-border pt-3">
                <Button
                  variant="outline"
                  size="sm"
                  type="button"
                  disabled={blocked || candidate.state !== "pending"}
                  onClick={() => {
                    setConflict(false);
                    setReviewing(candidate);
                  }}
                >
                  {text(
                    "Review and edit before admission",
                    "内容を確認・編集して採用",
                  )}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  className="hover:text-destructive"
                  disabled={
                    blocked ||
                    !["pending", "invalidated"].includes(candidate.state)
                  }
                  onClick={() =>
                    operate({
                      action: "review",
                      id: candidate.id,
                      expected_revision: candidate.revision,
                      mutation: null,
                    })
                  }
                >
                  {text("Reject", "却下")}
                </Button>
              </div>
            </article>
          ))}
        </Panel>
      )}
      {inspection.data && !inspection.error && (
        <Disclosure
          className="border-t border-border pt-3"
          summary={text(
            "Freshness, history, affected Runs and cleanup",
            "鮮度・履歴・影響するRun・消去状況",
          )}
        >
            <MemoryRecord
              value={{
                ...inspection.data.pages[0].value,
                items: inspection.data.pages.flatMap(
                  (page) => page.value.items,
                ),
              }}
            />
            {inspection.hasNextPage && (
              <div>
                <Button
                  variant="outline"
                  size="sm"
                  type="button"
                  disabled={blocked || inspection.isFetchingNextPage}
                  onClick={() => void inspection.fetchNextPage()}
                >
                  {text("More unit status", "さらにUnitの状態を表示")}
                </Button>
              </div>
            )}
        </Disclosure>
      )}
      {inspection.error && (
        <Hint role="status">
          {text(
            "History and cleanup status are unavailable with the current authority.",
            "現在の権限では履歴・消去状況を取得できません。",
          )}
        </Hint>
      )}
      {jobs.data && !jobs.error && (
        <Disclosure
          className="border-t border-border pt-3"
          summary={text("Engine jobs and retries", "メモリ処理・再試行")}
        >
            <MemoryRecord
              value={jobs.data.pages.flatMap((page) => page.value.items)}
            />
            {jobs.hasNextPage && (
              <div>
                <Button
                  variant="outline"
                  size="sm"
                  type="button"
                  onClick={() => void jobs.fetchNextPage()}
                >
                  {text("More jobs", "処理をさらに表示")}
                </Button>
              </div>
            )}
        </Disclosure>
      )}
      {questionSource && (
        <Modal
          title={text("Recurring question", "定期的な問い")}
          close={() => setQuestionSource(null)}
        >
          <form
            className="grid gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              const operation = crypto.randomUUID();
              const sources: Evidence[] = (units.data ?? [])
                .filter(
                  (unit) =>
                    questionSources.includes(unit.id) &&
                    unit.id !== questionTarget?.id,
                )
                .map((unit) => ({
                  kind: "unit",
                  bank: unit.bank,
                  id: unit.id,
                  revision: unit.revision,
                }));
              if (!sources.length) return;
              const content: Content = {
                ...questionSource.content,
                kind: "mental_model",
                verification: "unverified",
                text: question,
                mental_model: { question, automatic_refresh: autoRefresh },
                evidence: sources,
                links: [],
              };
              void send(`${base}/operate`, {
                ...read,
                operation_id: operation,
                action: {
                  action: "derive",
                  kind: "mental_model",
                  sources,
                  mutation: {
                    ...read,
                    operation_id: operation,
                    changes: [
                      questionTarget
                        ? {
                            operation: "correct",
                            id: questionTarget.id,
                            expected_revision: questionTarget.revision,
                            content,
                          }
                        : {
                            operation: "add",
                            id: crypto.randomUUID(),
                            content,
                          },
                    ],
                  },
                },
              });
            }}
          >
            <Field label={text("Question", "問い")}>
              <Textarea
                required
                value={question}
                onChange={(e) => setQuestion(e.target.value)}
              />
            </Field>
            <Group
              nested
              disabled={blocked}
              legend={text(
                "Select admitted evidence at its current revision",
                "採用済みの根拠を現在のrevisionで選択",
              )}
            >
              {(units.data ?? [])
                .filter((unit) => unit.id !== questionTarget?.id)
                .map((unit) => (
                  <Check
                    key={unit.id}
                      checked={questionSources.includes(unit.id)}
                      onChange={(e) =>
                        setQuestionSources((ids) =>
                          e.target.checked
                            ? [...ids, unit.id]
                            : ids.filter((id) => id !== unit.id),
                        )
                      }
                  >
                    {unit.content.text}{" "}
                    <span className="font-mono text-faint">
                      · r{unit.revision}
                    </span>
                  </Check>
                ))}
            </Group>
            <Check
                checked={autoRefresh}
                onChange={(e) => setAutoRefresh(e.target.checked)}
            >
              {text(
                "Refresh when admitted sources change",
                "採用済みの根拠が変わったら更新",
              )}
            </Check>
            <div className="flex justify-end">
              <Button
                disabled={
                  blocked ||
                  !question.trim() ||
                  !(units.data ?? []).some(
                    (unit) =>
                      questionSources.includes(unit.id) &&
                      unit.id !== questionTarget?.id,
                  )
                }
              >
                {questionTarget
                  ? text("Refresh with cited evidence", "根拠付きで更新")
                  : text("Create with cited evidence", "根拠付きで作成")}
              </Button>
            </div>
          </form>
          {modalNotice}
        </Modal>
      )}
      {historyUnit && (
        <Modal
          title={text("Unit history", "Unitの履歴")}
          close={() => setHistoryUnit(null)}
        >
          {historyCurrent && !history.error ? (
            <>
              <MemoryRecord
                value={history.data?.pages.flatMap((page) => page.value) ?? []}
              />
              {history.hasNextPage && (
                <div>
                  <Button
                    variant="outline"
                    size="sm"
                    type="button"
                    disabled={history.isFetchingNextPage}
                    onClick={() => void history.fetchNextPage()}
                  >
                    {text("Older revisions", "以前のrevisionを表示")}
                  </Button>
                </div>
              )}
            </>
          ) : (
            <Hint role="status">
              {text(
                "The observed unit or its disclosure changed. Refresh and inspect the current record.",
                "Unitまたは公開権限が変わりました。再読み込みして現在の記録を確認してください。",
              )}
            </Hint>
          )}
        </Modal>
      )}
      {reviewing && (
        <Modal
          title={text("Review the candidate", "学習候補のレビュー")}
          close={() => setReviewing(null)}
        >
          <UnitEditor
            bank={bank}
            key={reviewing.id}
            initial={reviewing.content}
            disabled={blocked || conflict}
            save={(content) => {
              const operation = crypto.randomUUID();
              void send(`${base}/operate`, {
                ...read,
                operation_id: operation,
                action: {
                  action: "review",
                  id: reviewing.id,
                  expected_revision: reviewing.revision,
                  mutation: {
                    ...read,
                    operation_id: operation,
                    changes: [
                      { operation: "add", id: crypto.randomUUID(), content },
                    ],
                  },
                },
              });
            }}
          />
          {modalNotice}
        </Modal>
      )}
      {editing && (
        <Modal
          title={text("Memory unit", "記憶Unit")}
          close={() => setEditing(null)}
        >
          <UnitEditor
            bank={bank}
            key={
              editing === "new" ? "new" : `${editing.id}:${editing.revision}`
            }
            unit={editing === "new" ? undefined : editing}
            disabled={blocked || conflict}
            save={(content) =>
              change(
                editing === "new"
                  ? { operation: "add", id: crypto.randomUUID(), content }
                  : {
                      operation: "correct",
                      id: editing.id,
                      expected_revision: editing.revision,
                      content,
                    },
              )
            }
          />
          {modalNotice}
        </Modal>
      )}
    </Panel>
  );
}
function UnitEditor({
  unit,
  bank,
  initial = unit?.content,
  disabled,
  save,
}: {
  unit?: Unit;
  bank: Bank;
  initial?: Content;
  disabled: boolean;
  save: (content: Content) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [text, setText] = useState(initial?.text ?? "");
  const [kind, setKind] = useState<Content["kind"]>(initial?.kind ?? "world");
  const [learning, setLearning] = useState<Content["learning"]>(
    initial?.learning ?? "fact",
  );
  const [question, setQuestion] = useState(
    initial?.mental_model?.question ?? "",
  );
  const [autoRefresh, setAutoRefresh] = useState(
    initial?.mental_model?.automatic_refresh ?? false,
  );
  const [metadata, setMetadata] = useState<
    Pick<Content, "occurred" | "entities" | "evidence" | "links">
  >({
    occurred: initial?.occurred ?? null,
    entities: initial?.entities ?? [],
    evidence: initial?.evidence ?? [],
    links: initial?.links ?? [],
  });
  const derived = kind === "observation" || kind === "mental_model";
  const [verification, setVerification] = useState<Content["verification"]>(
    initial?.verification ?? "unverified",
  );
  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        save({
          text,
          kind,
          learning,
          verification: derived ? "unverified" : verification,
          mental_model:
            kind === "mental_model"
              ? { question, automatic_refresh: autoRefresh }
              : null,
          ...metadata,
        });
      }}
    >
      <Field label={ja ? "内容" : "Content"}>
        <Textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          required
          maxLength={32768}
          className="min-h-28"
        />
      </Field>
      <div className="grid gap-3 sm:grid-cols-3">
        <Field label={ja ? "種類" : "Kind"}>
          <NativeSelect
            value={kind}
            disabled={derived}
            onChange={(e) => setKind(e.target.value as Content["kind"])}
          >
            {derived && <option value={kind}>{kind}</option>}
            <option value="world">{ja ? "世界に関する記憶" : "World"}</option>
            <option value="experience">
              {ja ? "Agentの経験" : "Experience"}
            </option>
          </NativeSelect>
        </Field>
        <Field label={ja ? "学習の種類" : "Learning type"}>
          <NativeSelect
            value={learning}
            onChange={(e) => setLearning(e.target.value as Content["learning"])}
          >
            {["fact", "preference", "procedure", "failure"].map((value) => (
              <option key={value} value={value}>
                {(
                  {
                    fact: "事実に関する主張",
                    preference: "好み",
                    procedure: "手順",
                    failure: "失敗",
                  } as Record<string, string>
                )[value] && ja
                  ? (
                      {
                        fact: "事実に関する主張",
                        preference: "好み",
                        procedure: "手順",
                        failure: "失敗",
                      } as Record<string, string>
                    )[value]
                  : value}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label={ja ? "検証状態" : "Verification"}>
          <NativeSelect
            value={derived ? "unverified" : verification}
            disabled={derived}
            onChange={(e) =>
              setVerification(e.target.value as Content["verification"])
            }
          >
            <option value="unverified">{ja ? "未検証" : "Unverified"}</option>
            <option value="supported">
              {ja ? "根拠を確認済み" : "Supported"}
            </option>
            <option value="contradicted">
              {ja ? "矛盾あり" : "Contradicted"}
            </option>
          </NativeSelect>
        </Field>
      </div>
      {kind === "mental_model" && (
        <>
          <Field label={ja ? "問い" : "Recurring question"}>
            <Textarea
              required
              readOnly
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
            />
          </Field>
          <Check
              disabled
              checked={autoRefresh}
              onChange={(e) => setAutoRefresh(e.target.checked)}
          >
            {ja ? "根拠の変更時に更新" : "Refresh after source changes"}
          </Check>
        </>
      )}
      <ContentMetadata
        bank={bank}
        value={metadata}
        change={setMetadata}
        disabled={disabled}
      />
      <div className="flex justify-end">
        <Button
          disabled={
            disabled ||
            !text.trim() ||
            (verification === "supported" && metadata.evidence.length === 0)
          }
        >
          {ja ? "確認したrevisionを保存" : "Save the observed revision"}
        </Button>
      </div>
    </form>
  );
}

function ContentMetadata({
  bank,
  value,
  change,
  disabled,
}: {
  bank: Bank;
  value: Pick<Content, "occurred" | "entities" | "evidence" | "links">;
  change: (
    value: Pick<Content, "occurred" | "entities" | "evidence" | "links">,
  ) => void;
  disabled: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const label = (en: string, jp: string) => (ja ? jp : en);
  const dateValue = (iso: string) => iso.slice(0, 16);
  return (
    <Group
      nested
      disabled={disabled}
      legend={label(
        "Sources, occurrence time and relationships",
        "出典・発生時刻・関係",
      )}
    >
      <Check
          checked={value.occurred !== null}
          onChange={(e) =>
            change({
              ...value,
              occurred: e.target.checked
                ? {
                    start: new Date().toISOString(),
                    end: new Date().toISOString(),
                  }
                : null,
            })
          }
      >
        {label(
          "Record an occurrence interval (UTC)",
          "発生時刻の範囲を記録（UTC）",
        )}
      </Check>
      {value.occurred && (
        <div className="grid gap-3 sm:grid-cols-2">
          {(["start", "end"] as const).map((key) => (
            <Field
              key={key}
              label={
                key === "start" ? label("Start", "開始") : label("End", "終了")
              }
            >
              <Input
                type="datetime-local"
                required
                value={dateValue(value.occurred![key])}
                min={
                  key === "end" ? dateValue(value.occurred!.start) : undefined
                }
                onChange={(e) => {
                  if (e.target.value)
                    change({
                      ...value,
                      occurred: {
                        ...value.occurred!,
                        [key]: new Date(`${e.target.value}:00Z`).toISOString(),
                      },
                    });
                }}
              />
            </Field>
          ))}
        </div>
      )}
      <h4 className={subheading}>{label("Entities", "エンティティ")}</h4>
      {value.entities.map((entity, index) => (
        <div
          className="grid items-end gap-3 rounded-md bg-raised/50 p-3 sm:grid-cols-2"
          key={index}
        >
          <Field label={label("Name", "名前")}>
            <Input
              required
              value={entity.name}
              onChange={(e) =>
                change({
                  ...value,
                  entities: value.entities.map((item, i) =>
                    i === index ? { ...item, name: e.target.value } : item,
                  ),
                })
              }
            />
          </Field>
          <Field label={label("Category", "分類")}>
            <Input
              required
              value={entity.category}
              onChange={(e) =>
                change({
                  ...value,
                  entities: value.entities.map((item, i) =>
                    i === index ? { ...item, category: e.target.value } : item,
                  ),
                })
              }
            />
          </Field>
          <Field label={label("Aliases (one per line)", "別名（1行に1つ）")}>
            <Textarea
              value={(entity.aliases ?? []).join("\n")}
              onChange={(event) =>
                change({
                  ...value,
                  entities: value.entities.map((item, i) =>
                    i === index
                      ? {
                          ...item,
                          aliases: event.target.value
                            .split("\n")
                            .filter((alias) => alias.trim().length > 0),
                        }
                      : item,
                  ),
                })
              }
            />
          </Field>
          <Button
            variant="ghost"
            size="sm"
            className="justify-self-start hover:text-destructive"
            type="button"
            onClick={() =>
              change({
                ...value,
                entities: value.entities.filter((_, i) => i !== index),
              })
            }
          >
            {label("Remove entity", "エンティティを削除")}
          </Button>
        </div>
      ))}
      <Button
        variant="outline"
        size="sm"
        className="justify-self-start"
        type="button"
        onClick={() =>
          change({
            ...value,
            entities: [...value.entities, { name: "", category: "" }],
          })
        }
      >
        {label("Add entity", "エンティティを追加")}
      </Button>
      <h4 className={subheading}>
        {label("Exact source revisions", "出典の正確なrevision")}
      </h4>
      {value.evidence.map((source, index) => (
        <div
          className="grid items-end gap-3 rounded-md bg-raised/50 p-3 sm:grid-cols-2"
          key={index}
        >
          <Field label={label("Source kind", "出典の種類")}>
            <NativeSelect
              value={source.kind}
              onChange={(e) => {
                const kind = e.target.value;
                const next: Evidence = {
                  kind,
                  id: source.id,
                  revision: source.revision,
                  ...(kind === "unit"
                    ? { bank }
                    : kind !== "publication"
                      ? { digest: "" }
                      : {}),
                };
                change({
                  ...value,
                  evidence: value.evidence.map((item, i) =>
                    i === index ? next : item,
                  ),
                });
              }}
            >
              {["unit", "message", "artifact", "run", "publication"].map(
                (kind) => (
                  <option key={kind}>{kind}</option>
                ),
              )}
            </NativeSelect>
          </Field>
          <Field label={label("Source ID", "出典ID")}>
            <Input
              required
              value={source.id}
              onChange={(e) =>
                change({
                  ...value,
                  evidence: value.evidence.map((item, i) =>
                    i === index ? { ...item, id: e.target.value } : item,
                  ),
                })
              }
            />
          </Field>
          <Field label="Revision">
            <Input
              type="number"
              required
              min={1}
              max={Number.MAX_SAFE_INTEGER}
              value={source.revision}
              onChange={(e) =>
                change({
                  ...value,
                  evidence: value.evidence.map((item, i) =>
                    i === index
                      ? { ...item, revision: Number(e.target.value) }
                      : item,
                  ),
                })
              }
            />
          </Field>
          {source.bank && (
            <small>
              {label(
                "Home / Workspace / participant",
                "Home／Workspace／所有者",
              )}
              : {source.bank.home} / {source.bank.workspace} /{" "}
              {source.bank.participant ?? label("Shared", "共有")}
            </small>
          )}
          {source.digest !== undefined && (
            <Field
              label={label(
                "SHA-256 content digest",
                "本文のSHA-256ダイジェスト",
              )}
            >
              <Input
                required
                pattern="[a-f0-9]{64}"
                value={source.digest}
                onChange={(e) =>
                  change({
                    ...value,
                    evidence: value.evidence.map((item, i) =>
                      i === index ? { ...item, digest: e.target.value } : item,
                    ),
                  })
                }
              />
            </Field>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="justify-self-start hover:text-destructive"
            type="button"
            onClick={() =>
              change({
                ...value,
                evidence: value.evidence.filter((_, i) => i !== index),
              })
            }
          >
            {label("Remove source", "出典を削除")}
          </Button>
        </div>
      ))}
      <Button
        variant="outline"
        size="sm"
        className="justify-self-start"
        type="button"
        onClick={() =>
          change({
            ...value,
            evidence: [
              ...value.evidence,
              { kind: "unit", bank, id: "", revision: 1 },
            ],
          })
        }
      >
        {label("Add exact source", "正確な出典を追加")}
      </Button>
      <h4 className={subheading}>{label("Relationships", "関係")}</h4>
      {value.links.map((link, index) => (
        <div
          className="grid items-end gap-3 rounded-md bg-raised/50 p-3 sm:grid-cols-2"
          key={index}
        >
          <Field label={label("Target unit ID", "関係先Unit ID")}>
            <Input
              required
              value={link.target}
              onChange={(e) =>
                change({
                  ...value,
                  links: value.links.map((item, i) =>
                    i === index ? { ...item, target: e.target.value } : item,
                  ),
                })
              }
            />
          </Field>
          <Field label="Revision">
            <Input
              type="number"
              required
              min={1}
              max={Number.MAX_SAFE_INTEGER}
              value={link.revision}
              onChange={(e) =>
                change({
                  ...value,
                  links: value.links.map((item, i) =>
                    i === index
                      ? { ...item, revision: Number(e.target.value) }
                      : item,
                  ),
                })
              }
            />
          </Field>
          <Field label={label("Relationship", "関係の種類")}>
            <NativeSelect
              value={link.kind}
              onChange={(e) =>
                change({
                  ...value,
                  links: value.links.map((item, i) =>
                    i === index ? { ...item, kind: e.target.value } : item,
                  ),
                })
              }
            >
              {["entity", "semantic", "temporal", "causes", "caused_by"].map(
                (kind) => (
                  <option key={kind}>{kind}</option>
                ),
              )}
            </NativeSelect>
          </Field>
          <Field label={label("Weight", "重み")}>
            <Input
              type="number"
              required
              min={0}
              max={1}
              step={0.01}
              value={link.weight}
              onChange={(e) =>
                change({
                  ...value,
                  links: value.links.map((item, i) =>
                    i === index
                      ? { ...item, weight: Number(e.target.value) }
                      : item,
                  ),
                })
              }
            />
          </Field>
          <Button
            variant="ghost"
            size="sm"
            className="justify-self-start hover:text-destructive"
            type="button"
            onClick={() =>
              change({
                ...value,
                links: value.links.filter((_, i) => i !== index),
              })
            }
          >
            {label("Remove relationship", "関係を削除")}
          </Button>
        </div>
      ))}
      <Button
        variant="outline"
        size="sm"
        className="justify-self-start"
        type="button"
        onClick={() =>
          change({
            ...value,
            links: [
              ...value.links,
              { target: "", revision: 1, kind: "semantic", weight: 1 },
            ],
          })
        }
      >
        {label("Add relationship", "関係を追加")}
      </Button>
    </Group>
  );
}
