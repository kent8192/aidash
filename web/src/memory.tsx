import { useState } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { State, EntityRef } from "./types";
import { apiFetch, ApiError } from "./transport";
import { Badge, Empty, Field, Panel, Modal, useI18n } from "./ui";
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
  const boundProvider = agent?.config.memory as EntityRef | undefined;
  const provider = chosen
    ? boundProvider
    : providers.find((entry) => refKey(entry) === providerKey);
  const providerRef = provider
    ? { id: provider.id, version: provider.version }
    : undefined;
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
    <div role="alert">
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
        <button
          type="button"
          disabled={busy}
          onClick={() => void send(retry.url, retry.body)}
        >
          {text("Retry the same operation", "同じ操作を再試行")}
        </button>
      )}
    </div>
  );
  return (
    <Panel title={text("Hindsight memory", "Hindsight メモリ")}>
      <p className="muted">
        {text(
          "Private memory belongs to a logical Agent in this Workspace. Publish only selected units to share them.",
          "プライベート記憶はこのWorkspaceの論理Agentに属します。選択したUnitを公開すると共有記憶になります。",
        )}
      </p>
      {data.access.kind === "operator" && (
        <Field label={text("Tenant", "テナント")}>
          <input
            disabled={blocked}
            value={tenant}
            onChange={(e) => setTenant(e.target.value)}
          />
        </Field>
      )}
      <Field label={text("Memory bank", "記憶の所有者")}>
        <select
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
              {refKey(value.agent)} · {value.id.slice(0, 8)} · r{value.revision}
            </option>
          ))}
        </select>
      </Field>
      {participants.hasNextPage && (
        <button
          type="button"
          disabled={participants.isFetchingNextPage}
          onClick={() => void participants.fetchNextPage()}
        >
          {text("More Agents", "Agentをさらに表示")}
        </button>
      )}
      {!chosen && (
        <Field
          label={text(
            "Memory provider version",
            "メモリプロバイダのバージョン",
          )}
        >
          <select
            value={providerKey}
            disabled={blocked}
            onChange={(e) => setProviderKey(e.target.value)}
          >
            <option value="">
              {text("Choose a registered provider", "登録済みプロバイダを選択")}
            </option>
            {providers.map((value) => (
              <option key={refKey(value)} value={refKey(value)}>
                {local(value.name)} · {refKey(value)}
              </option>
            ))}
          </select>
        </Field>
      )}
      {chosen && (
        <p>
          {text("Accepted Agent and provider", "受け入れ済みAgentとプロバイダ")}
          : {refKey(chosen.agent)} ·{" "}
          {providerRef
            ? refKey(providerRef)
            : text("Memory disabled", "メモリ無効")}
        </p>
      )}
      <div className="form-grid">
        <Field label={text("Agent version", "Agentのバージョン")}>
          <select
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
          </select>
        </Field>
        <button
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
        </button>
        {chosen && (
          <button
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
          </button>
        )}
      </div>
      {chosen && (
        <div className="form-grid">
          <Field label={text("Task", "タスク")}>
            <select
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
            </select>
          </Field>
          <button
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
          </button>
          {assignment.data?.participant && (
            <>
              <p>
                {text("Current assignment", "現在の割当")}:{" "}
                {assignment.data.participant.participant_id.slice(0, 8)} · r
                {assignment.data.participant.participant_revision}
              </p>
              <button
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
              </button>
            </>
          )}
        </div>
      )}
      {!!read && !chosen && (
        <button
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
        </button>
      )}
      {(error || failures.length > 0) && (
        <div role="alert" className="error">
          {error || failures.map((value) => String(value)).join(" · ")}
          {conflict && (
            <p>
              {text(
                "The observed revision changed. Refresh and review the new state before editing again.",
                "確認したrevisionが変わりました。再読み込みして内容を確認し、改めて編集してください。",
              )}
            </p>
          )}
          {retry && (
            <>
              <button
                type="button"
                disabled={busy}
                onClick={() => void send(retry.url, retry.body)}
              >
                {text("Retry the same operation", "同じ操作を再試行")}
              </button>
              <button
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
              </button>
            </>
          )}
        </div>
      )}
      <button
        type="button"
        disabled={blocked}
        onClick={() => {
          setEditing(null);
          setConflict(false);
          void client.invalidateQueries({ queryKey: ["memory", workspace] });
        }}
      >
        {text("Refresh", "再読み込み")}
      </button>
      {!!read && (
        <>
          {chosen && (
            <Field
              label={text(
                "Shared publication policy",
                "公開先の共有メモリポリシー",
              )}
            >
              <select
                value={publicationProvider ? refKey(publicationProvider) : ""}
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
              </select>
            </Field>
          )}
          <button
            type="button"
            disabled={blocked}
            onClick={() => setEditing("new")}
          >
            {text("Add a memory unit", "Unitを追加")}
          </button>
          <button
            type="button"
            disabled={blocked}
            onClick={() => operate({ action: "usage", after: null })}
          >
            {text("Model usage and charged cost", "モデル使用量・計上コスト")}
          </button>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              operate({
                action: "recall",
                query: { text: query, time: null, kinds: [], max_tokens: 4096 },
              });
            }}
          >
            <Field label={text("Recall or reflect", "記憶の検索・考察")}>
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                required
              />
            </Field>
            <button disabled={blocked}>{text("Recall", "検索")}</button>
            <button
              type="button"
              disabled={blocked || !query.trim()}
              onClick={() =>
                operate({
                  action: "reflect",
                  query: {
                    text: query,
                    time: null,
                    kinds: [],
                    max_tokens: 4096,
                  },
                })
              }
            >
              {text("Reflect with cited evidence", "根拠付きで考察")}
            </button>
          </form>
        </>
      )}
      {units.data?.length === 0 && <Empty />}
      {!units.error && units.data && <MemoryOverview units={units.data} />}
      <div className="memory-unit-list">
        {!units.error &&
          units.data?.map((unit) => (
            <article
              className="memory-unit"
              id={unitAnchor(unit.id)}
              tabIndex={-1}
              key={unit.id}
            >
              <MemoryContent
                content={unit.content}
                unit={unit}
                units={units.error ? [] : (units.data ?? [])}
              />
              <button
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
              </button>
              {unit.content.mental_model && (
                <button
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
                </button>
              )}
              <button
                type="button"
                disabled={blocked}
                onClick={() => setHistoryUnit(unit)}
              >
                {text("History", "履歴")}
              </button>
              <button
                type="button"
                disabled={blocked}
                onClick={() => {
                  setConflict(false);
                  setEditing(unit);
                }}
              >
                {text("Correct", "訂正")}
              </button>
              <button
                type="button"
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
              </button>
              {chosen && (
                <button
                  type="button"
                  disabled={
                    blocked || !publicationProvider || sharedSettings.isLoading
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
                </button>
              )}
            </article>
          ))}
      </div>
      {candidates.data && !candidates.error && (
        <Panel
          title={text("Run learning review", "Runからの学習候補のレビュー")}
        >
          <p>
            {text(
              "Run completion does not verify a claim. Admission is a human decision.",
              "Runの完了は主張の正しさを証明しません。人が内容を確認して採用します。",
            )}
          </p>
          {candidates.data.value.map((candidate) => (
            <article key={candidate.id}>
              <Badge value={candidate.state} />
              <MemoryContent
                content={candidate.content}
                units={units.error ? [] : (units.data ?? [])}
              />
              <MemoryRecord
                value={{
                  source: candidate.run,
                  evidence: candidate.content.evidence,
                }}
              />
              <button
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
              </button>
              <button
                type="button"
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
              </button>
            </article>
          ))}
        </Panel>
      )}
      {inspection.data && !inspection.error && (
        <details>
          <summary>
            {text(
              "Freshness, history, affected Runs and cleanup",
              "鮮度・履歴・影響するRun・消去状況",
            )}
          </summary>
          <MemoryRecord
            value={{
              ...inspection.data.pages[0].value,
              items: inspection.data.pages.flatMap((page) => page.value.items),
            }}
          />
          {inspection.hasNextPage && (
            <button
              type="button"
              disabled={blocked || inspection.isFetchingNextPage}
              onClick={() => void inspection.fetchNextPage()}
            >
              {text("More unit status", "さらにUnitの状態を表示")}
            </button>
          )}
        </details>
      )}
      {inspection.error && (
        <p role="status">
          {text(
            "History and cleanup status are unavailable with the current authority.",
            "現在の権限では履歴・消去状況を取得できません。",
          )}
        </p>
      )}
      {jobs.data && !jobs.error && (
        <details>
          <summary>
            {text("Engine jobs and retries", "メモリ処理・再試行")}
          </summary>
          <MemoryRecord
            value={jobs.data.pages.flatMap((page) => page.value.items)}
          />
          {jobs.hasNextPage && (
            <button type="button" onClick={() => void jobs.fetchNextPage()}>
              {text("More jobs", "処理をさらに表示")}
            </button>
          )}
        </details>
      )}
      {questionSource && (
        <Modal
          title={text("Recurring question", "定期的な問い")}
          close={() => setQuestionSource(null)}
        >
          <form
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
              <textarea
                required
                value={question}
                onChange={(e) => setQuestion(e.target.value)}
              />
            </Field>
            <fieldset disabled={blocked}>
              <legend>
                {text(
                  "Select admitted evidence at its current revision",
                  "採用済みの根拠を現在のrevisionで選択",
                )}
              </legend>
              {(units.data ?? [])
                .filter((unit) => unit.id !== questionTarget?.id)
                .map((unit) => (
                  <label key={unit.id}>
                    <input
                      type="checkbox"
                      checked={questionSources.includes(unit.id)}
                      onChange={(e) =>
                        setQuestionSources((ids) =>
                          e.target.checked
                            ? [...ids, unit.id]
                            : ids.filter((id) => id !== unit.id),
                        )
                      }
                    />
                    {unit.content.text} · r{unit.revision}
                  </label>
                ))}
            </fieldset>
            <label>
              <input
                type="checkbox"
                checked={autoRefresh}
                onChange={(e) => setAutoRefresh(e.target.checked)}
              />
              {text(
                "Refresh when admitted sources change",
                "採用済みの根拠が変わったら更新",
              )}
            </label>
            <button
              className="primary"
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
            </button>
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
                <button
                  type="button"
                  disabled={history.isFetchingNextPage}
                  onClick={() => void history.fetchNextPage()}
                >
                  {text("Older revisions", "以前のrevisionを表示")}
                </button>
              )}
            </>
          ) : (
            <p role="status">
              {text(
                "The observed unit or its disclosure changed. Refresh and inspect the current record.",
                "Unitまたは公開権限が変わりました。再読み込みして現在の記録を確認してください。",
              )}
            </p>
          )}
        </Modal>
      )}
      {result && failures.length === 0 && !inspection.error && (
        <details open>
          <summary>{text("Operation result", "操作結果")}</summary>
          <MemoryRecord value={result} />
        </details>
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
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          required
          maxLength={32768}
        />
      </Field>
      <Field label={ja ? "種類" : "Kind"}>
        <select
          value={kind}
          disabled={derived}
          onChange={(e) => setKind(e.target.value as Content["kind"])}
        >
          {derived && <option value={kind}>{kind}</option>}
          <option value="world">{ja ? "世界に関する記憶" : "World"}</option>
          <option value="experience">
            {ja ? "Agentの経験" : "Experience"}
          </option>
        </select>
      </Field>
      <Field label={ja ? "学習の種類" : "Learning type"}>
        <select
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
        </select>
      </Field>
      {kind === "mental_model" && (
        <>
          <Field label={ja ? "問い" : "Recurring question"}>
            <textarea
              required
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
            />
          </Field>
          <label>
            <input
              type="checkbox"
              checked={autoRefresh}
              onChange={(e) => setAutoRefresh(e.target.checked)}
            />
            {ja ? "根拠の変更時に更新" : "Refresh after source changes"}
          </label>
        </>
      )}
      <Field label={ja ? "検証状態" : "Verification"}>
        <select
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
        </select>
      </Field>
      <ContentMetadata
        bank={bank}
        value={metadata}
        change={setMetadata}
        disabled={disabled}
      />
      <button
        className="primary"
        disabled={
          disabled ||
          !text.trim() ||
          (verification === "supported" && metadata.evidence.length === 0)
        }
      >
        {ja ? "確認したrevisionを保存" : "Save the observed revision"}
      </button>
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
    <fieldset disabled={disabled}>
      <legend>
        {label(
          "Sources, occurrence time and relationships",
          "出典・発生時刻・関係",
        )}
      </legend>
      <label>
        <input
          type="checkbox"
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
        />
        {label(
          "Record an occurrence interval (UTC)",
          "発生時刻の範囲を記録（UTC）",
        )}
      </label>
      {value.occurred && (
        <div className="form-grid">
          {(["start", "end"] as const).map((key) => (
            <Field
              key={key}
              label={
                key === "start" ? label("Start", "開始") : label("End", "終了")
              }
            >
              <input
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
      <h4>{label("Entities", "エンティティ")}</h4>
      {value.entities.map((entity, index) => (
        <div className="form-grid" key={index}>
          <Field label={label("Name", "名前")}>
            <input
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
            <input
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
            <textarea
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
          <button
            type="button"
            onClick={() =>
              change({
                ...value,
                entities: value.entities.filter((_, i) => i !== index),
              })
            }
          >
            {label("Remove entity", "エンティティを削除")}
          </button>
        </div>
      ))}
      <button
        type="button"
        onClick={() =>
          change({
            ...value,
            entities: [...value.entities, { name: "", category: "" }],
          })
        }
      >
        {label("Add entity", "エンティティを追加")}
      </button>
      <h4>{label("Exact source revisions", "出典の正確なrevision")}</h4>
      {value.evidence.map((source, index) => (
        <div className="form-grid" key={index}>
          <Field label={label("Source kind", "出典の種類")}>
            <select
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
            </select>
          </Field>
          <Field label={label("Source ID", "出典ID")}>
            <input
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
            <input
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
              <input
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
          <button
            type="button"
            onClick={() =>
              change({
                ...value,
                evidence: value.evidence.filter((_, i) => i !== index),
              })
            }
          >
            {label("Remove source", "出典を削除")}
          </button>
        </div>
      ))}
      <button
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
      </button>
      <h4>{label("Relationships", "関係")}</h4>
      {value.links.map((link, index) => (
        <div className="form-grid" key={index}>
          <Field label={label("Target unit ID", "関係先Unit ID")}>
            <input
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
            <input
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
            <select
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
            </select>
          </Field>
          <Field label={label("Weight", "重み")}>
            <input
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
          <button
            type="button"
            onClick={() =>
              change({
                ...value,
                links: value.links.filter((_, i) => i !== index),
              })
            }
          >
            {label("Remove relationship", "関係を削除")}
          </button>
        </div>
      ))}
      <button
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
      </button>
    </fieldset>
  );
}
