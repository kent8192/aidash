import { ProviderToolActivity } from "../provider-tools";
import {
  Alert,
  Group,
  Hint,
  Notice,
  Pre,
  inlineFormClass,
} from "../components/patterns";
import { NativeSelect } from "../components/ui/native-select";
import { Textarea } from "../components/ui/textarea";
import { Button } from "../components/ui/button";
import { useRef, useState } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import type { State } from "../types";
import { post, type Area, type Operation } from "./client";
import {
  ApprovalCardView,
  CapabilityApprovals,
  useThreadApprovalList,
} from "./approvals";
import { usePendingApprovalInbox } from "./inbox";
import { SkillFiles } from "./skills";
import { DisplayFile, panelClass } from "./display";
import { LivePreviewPanel } from "./live-preview";
import { registryDeployment } from "../node-selection";
import { UrlIntentPicker } from "../url-intent-picker";
import {
  bindsPageReading,
  snapshotReadsPages,
  urlIntents,
} from "../url-intents";
import type { ApprovalCard as Approval, RunDetails } from "../generated/models";
type Session = {
  active_run_id: string | null;
  last_run_id: string | null;
  last_agent_version: string | null;
};
function OperationCard({
  run,
  operation,
  area,
  onSession,
}: {
  run: string;
  operation: Operation;
  area: Area;
  onSession: (id: string) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [offset, setOffset] = useState(0);
  const [error, setError] = useState("");
  const query = useQuery({
    queryKey: ["core-operation", run, operation.operation_id, offset],
    queryFn: () =>
      post<Operation>(
        `/executions/${run}/${operation.kind === "code_interpreter" ? "python" : operation.kind}/poll`,
        {
          operation_id: operation.operation_id,
          offset,
        },
      ),
    refetchInterval: (q) =>
      ["prepared", "submitted", "running", "cancelling"].includes(
        q.state.data?.status ?? operation.status,
      )
        ? 1000
        : false,
    retry: false,
  });
  const value = query.data ?? operation;
  return (
    <article>
      <h4>
        {operation.kind} · {value.status}
      </h4>
      {value.session_id && <Hint>Python session: {value.session_id}</Hint>}
      {value.output && <Pre>{value.output}</Pre>}
      {value.displays?.map((file) => (
        <DisplayFile
          key={file.file_id}
          area={value.area_id ?? area.id}
          file={file}
          files={area.manifest}
          revision={area.revision}
        />
      ))}
      {(query.error || error || value.error) && (
        <Alert>{query.error?.message || error || value.error?.message}</Alert>
      )}
      {value.effects_may_have_occurred && (
        <Notice tone="warning">
          {ja
            ? "既に行われた変更や外部処理は取り消されていない可能性があります。"
            : "Changes or external effects may already have occurred."}
        </Notice>
      )}
      <div className={inlineFormClass}>
        {value.next_offset !== null && value.next_offset !== undefined && (
          <Button
            variant="outline"
            type="button"
            onClick={() => setOffset(value.next_offset!)}
          >
            {ja ? "出力の続き" : "More output"}
          </Button>
        )}
        {["prepared", "submitted", "running", "cancelling"].includes(
          value.status,
        ) && (
          <Button
            variant="outline"
            type="button"
            onClick={async () => {
              try {
                await post(
                  `/executions/${run}/${operation.kind === "code_interpreter" ? "python" : operation.kind}/cancel`,
                  {
                    operation_id: operation.operation_id,
                  },
                );
                await query.refetch();
              } catch (e) {
                setError(String(e));
              }
            }}
          >
            {ja ? "実行を停止" : "Stop execution"}
          </Button>
        )}
        {value.session_id && (
          <Button
            variant="outline"
            type="button"
            onClick={() => onSession(value.session_id!)}
          >
            {ja ? "この Python セッションを使用" : "Use this Python session"}
          </Button>
        )}
      </div>
      {value.termination_confirmed && (
        <Hint>
          {ja ? "プロセスの停止を確認済み" : "Process termination confirmed"}
        </Hint>
      )}
      {value.writer_frozen && (
        <Hint>
          {ja
            ? "Python の状態を保存して待機中"
            : "Python state retained; execution frozen"}
        </Hint>
      )}
    </article>
  );
}
function ScratchOperations({
  area,
  run,
  refresh,
  inlineIds,
}: {
  area: Area;
  run: string;
  refresh: () => Promise<unknown>;
  inlineIds: ReadonlySet<string>;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [code, setCode] = useState("");
  const [kind, setKind] = useState("python");
  const [session, setSession] = useState<string>();
  const [reset, setReset] = useState<Operation>();
  const [patch, setPatch] = useState("");
  const pending = useRef<{ signature: string; key: string } | undefined>(
    undefined,
  );
  const mutate = <T,>(path: string, input: Record<string, unknown>) => {
    const signature = JSON.stringify([path, input]);
    if (pending.current?.signature !== signature)
      pending.current = { signature, key: crypto.randomUUID() };
    return post<T>(path, { ...input, idempotency_key: pending.current.key });
  };
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await action();
      pending.current = undefined;
      await refresh();
      await client.invalidateQueries({
        queryKey: ["core-operation-history", run],
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const history = useInfiniteQuery({
    queryKey: ["core-operation-history", run],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{ items: Operation[]; next_cursor: string | null }>(
        `/api/executions/${run}/core-operations${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const runCode = async (expected_session_id?: string) => {
    const result = await mutate<Operation>(`/executions/${run}/${kind}`, {
      expected_revision: area.revision,
      ...(kind === "python"
        ? { code, expected_session_id: expected_session_id ?? session ?? null }
        : { command: code }),
    });
    if (result.session_reset) setReset(result);
    else {
      setReset(undefined);
      if (result.session_id) setSession(result.session_id);
    }
  };
  return (
    <>
      <details className={panelClass}>
        <summary>Shell · Python</summary>
        <Field label={ja ? "実行する言語" : "Runtime"}>
          <NativeSelect value={kind} onChange={(e) => setKind(e.target.value)}>
            <option value="python">Python</option>
            <option value="shell">Shell</option>
          </NativeSelect>
        </Field>
        <Field label={ja ? "コード" : "Code"}>
          <Textarea
            value={code}
            onChange={(e) => setCode(e.target.value)}
            spellCheck={false}
          />
        </Field>
        <Button
          variant="outline"
          type="button"
          disabled={busy || !code.trim()}
          onClick={() => void act(() => runCode())}
        >
          {ja ? "実行" : "Execute"}
        </Button>
        {reset && (
          <article>
            <Notice tone="warning">
              {ja
                ? "Python のメモリはリセットされました。変数は復元されません。保存済みファイルを確認してから再実行してください。"
                : "Python memory was reset. Variables were not restored. Review the saved files before running again."}
            </Notice>
            <Hint>{reset.reset_reason}</Hint>
            <Button
              variant="outline"
              type="button"
              disabled={busy}
              onClick={() => void act(() => runCode(reset.session_id))}
            >
              {ja
                ? "リセットを確認してこのコードを実行"
                : "Acknowledge reset and execute this code"}
            </Button>
          </article>
        )}
        {history.isError && <Alert>{history.error.message}</Alert>}
        {history.data?.pages
          .flatMap((p) => p.items)
          .map((op) => (
            <OperationCard
              key={op.operation_id}
              run={run}
              operation={op}
              area={area}
              onSession={setSession}
            />
          ))}
        {history.hasNextPage && (
          <Button
            variant="outline"
            type="button"
            onClick={() => void history.fetchNextPage()}
          >
            {ja ? "さらに表示" : "Load more"}
          </Button>
        )}
      </details>
      <details className={panelClass}>
        <summary>
          {ja ? "パッチの確認と適用" : "Review and apply a patch"}
        </summary>
        <Field label="Patch">
          <Textarea
            value={patch}
            onChange={(e) => {
              setPatch(e.target.value);
            }}
            spellCheck={false}
          />
        </Field>
        <Button
          variant="outline"
          type="button"
          disabled={busy || !patch.trim()}
          onClick={() =>
            void act(() => mutate(`/executions/${run}/patch`, { patch }))
          }
        >
          {ja ? "パッチを適用" : "Apply patch"}
        </Button>
      </details>
      <SkillFiles run={run} />
      <CapabilityApprovals area={area.id} inlineIds={inlineIds} />
      {error && <Alert>{error}</Alert>}
    </>
  );
}
export function ThreadCapabilities({
  thread,
  data,
  onDeleted,
}: {
  thread: string;
  data: State;
  onDeleted: () => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [selected, setSelected] = useState("");
  const [prompt, setPrompt] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [agent, setAgent] = useState("");
  const [readPages, setReadPages] = useState<string[]>([]);
  const [requested, setRequested] = useState<string[]>([]);
  const pendingEnqueue = useRef<{ signature: string; key: string } | undefined>(
    undefined,
  );
  const pendingSteer = useRef<{ signature: string; key: string } | undefined>(
    undefined,
  );
  const query = useQuery({
    queryKey: ["core-areas", thread],
    queryFn: async ({ signal }) => {
      const items: Area[] = [];
      let cursor: string | null = null;
      do {
        const page: { items: Area[]; next_cursor: string | null } =
          await apiFetch(
            `/api/working-areas?thread_id=${thread}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`,
            { signal },
          );
        items.push(...page.items);
        cursor = page.next_cursor;
      } while (cursor);
      return { items };
    },
    refetchInterval: 2000,
    retry: false,
  });
  const area =
    query.data?.items.find((a) => a.id === selected) ?? query.data?.items[0];
  // Follow the shell inbox's and the Thread list's caches without adding a
  // polling observer or fetching every page of approval history.
  const inbox = usePendingApprovalInbox(false);
  const list = useThreadApprovalList(false);
  const session = useQuery({
    queryKey: ["core-session", area?.id],
    queryFn: ({ signal }) =>
      apiFetch<Session>(`/api/working-areas/${area!.id}/session`, { signal }),
    enabled: !!area,
    refetchInterval: 2000,
    retry: false,
  });
  const active = session.data?.active_run_id ?? session.data?.last_run_id;
  // Whichever poll sees a card first offers it inline, so a card never moves
  // from the Thread's list to the top of the panel before a decision. The
  // list's copy wins because it polls more often, but only while the list is
  // shown: nothing refreshes its cache otherwise, not even a decision.
  const known = new Map<string, Approval>();
  for (const item of inbox.data ?? []) known.set(item.id, item);
  if (area && active)
    for (const item of list.data?.pages.flatMap((p) => p.items) ?? [])
      known.set(item.id, item);
  const areaIds = new Set(query.data?.items.map((item) => item.id) ?? []);
  const inlineApprovals = [...known.values()].filter(
    (item) =>
      item.viewer_can_decide_inline &&
      item.state === "pending" &&
      ["outbound", "tool_call"].includes(item.kind) &&
      item.area_id != null &&
      areaIds.has(item.area_id),
  );
  const inlineIds = new Set(inlineApprovals.map((item) => item.id));
  const agents = data.registry.filter(
    (e) => e.kind === "agent" && e.config?.schema_version === 1,
  );
  const refresh = () =>
    Promise.all([query.refetch(), ...(area ? [session.refetch()] : [])]);
  const act = async (f: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await f();
      await refresh();
      await client.invalidateQueries({ queryKey: ["state"] });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const enqueueRun = async (path: string, input: Record<string, unknown>) => {
    const pages = selectedPages.filter((page) => queued.includes(page));
    const body = pages.length ? { ...input, read_pages: pages } : input;
    const signature = JSON.stringify([path, body]);
    if (pendingEnqueue.current?.signature !== signature)
      pendingEnqueue.current = { signature, key: crypto.randomUUID() };
    await post(path, {
      ...body,
      idempotency_key: pendingEnqueue.current.key,
    });
    pendingEnqueue.current = undefined;
    setPrompt("");
    setRequested(pages);
    setReadPages([]);
  };
  const availableAgents = agents;
  const effectiveAgent = availableAgents.some(
    (e) => `${e.id}@${e.version}` === agent,
  )
    ? agent
    : area && session.data?.last_agent_version
      ? `${area.agent_id}@${session.data.last_agent_version}`
      : "";
  const target = availableAgents.find(
    (e) => `${e.id}@${e.version}` === effectiveAgent,
  );
  const registryNode = registryDeployment(data);
  const canRead = bindsPageReading(target, data.registry, registryNode);
  // Steering reaches the active Run, which reads pages only through the
  // Agent version it was pinned to, whichever version the selector shows.
  const activeRunId = area ? (session.data?.active_run_id ?? null) : null;
  const shownRun = useQuery({
    queryKey: ["run-agent", active],
    // The Run details carry the inspection under `run`.
    queryFn: async ({ signal }) => {
      const details = await apiFetch<RunDetails>(`/api/executions/${active}`, {
        signal,
      });
      return {
        ...details.run,
        provider_tool_activity: details.provider_tool_activity,
      };
    },
    enabled: !!active,
    refetchInterval: 2000,
    retry: false,
  });
  const pinned = agents.find(
    (e) =>
      e.id === shownRun.data?.agent_id &&
      e.version === shownRun.data?.agent_version,
  );
  const steeringCanRead = bindsPageReading(pinned, data.registry, registryNode);
  // The server records a steering URL intent only when the Run was admitted
  // with page reading, so a definition approved since then does not count.
  const activeContext = activeRunId ? shownRun.data?.context : undefined;
  const steerReads =
    steeringCanRead &&
    (!activeContext || snapshotReadsPages(activeContext.binding_snapshot));
  // A URL intent is offered only when an Agent it would reach can read pages
  // on this Node, and each action sends only the pages its Agent can read.
  const queued = canRead ? urlIntents(prompt) : [];
  const steered = activeRunId && steerReads ? urlIntents(prompt) : [];
  const intents = [...new Set([...queued, ...steered])];
  const selectedPages = readPages.filter((page) => intents.includes(page));
  // An action whose Agent cannot read a selected page is disabled rather
  // than sending the text without it.
  const queueReadsSelection = selectedPages.every((page) =>
    queued.includes(page),
  );
  const steerReadsSelection = selectedPages.every((page) =>
    steered.includes(page),
  );
  return (
    <section className={panelClass}>
      <h3>{ja ? "Agent の作業" : "Agent work"}</h3>
      {inlineApprovals.length > 0 && (
        <section
          aria-label={ja ? "このスレッドの承認" : "Approvals for this Thread"}
        >
          {inlineApprovals.map((item) => (
            <ApprovalCardView key={item.id} item={item} />
          ))}
        </section>
      )}
      {query.isError && <Alert>{query.error.message}</Alert>}
      {
        <Field label={ja ? "実行する Agent" : "Agent to run"}>
          <NativeSelect
            value={effectiveAgent}
            onChange={(e) => setAgent(e.target.value)}
          >
            <option value="">{ja ? "選択" : "Choose"}</option>
            {availableAgents.map((e) => (
              <option
                key={`${e.id}@${e.version}`}
              >{`${e.id}@${e.version}`}</option>
            ))}
          </NativeSelect>
        </Field>
      }
      {(query.data?.items.length ?? 0) > 1 && (
        <Field label={ja ? "Agent の作業履歴" : "Agent work history"}>
          <NativeSelect
            value={area?.id}
            onChange={(e) => setSelected(e.target.value)}
          >
            {query.data?.items.map((a) => (
              <option key={a.id} value={a.id}>
                {a.agent_id}
              </option>
            ))}
          </NativeSelect>
        </Field>
      )}
      <Field label={ja ? "次の指示" : "Next instruction"}>
        <Textarea
          disabled={busy}
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
        />
      </Field>
      <UrlIntentPicker
        candidates={intents}
        selected={selectedPages}
        onChange={setReadPages}
      />
      {requested.length > 0 && (
        <Hint role="status">
          {ja ? "読み取りを依頼しました" : "Asked the Run to read"}:{" "}
          {requested.join(", ")}
        </Hint>
      )}
      {(!queueReadsSelection || (activeRunId && !steerReadsSelection)) && (
        <Hint role="status">
          {ja
            ? "選んだページを読めない Agent への送信はできません。そのページの選択を外すと送れます。"
            : "An action whose Agent cannot read a selected page is unavailable; clear that page to use it."}
        </Hint>
      )}
      <div className={inlineFormClass}>
        <Button
          variant="outline"
          type="button"
          disabled={busy || !prompt.trim() || !target || !queueReadsSelection}
          onClick={() =>
            void act(async () => {
              if (target)
                await enqueueRun(
                  `/threads/${thread}/agents/${encodeURIComponent(target.id)}/executions`,
                  {
                    agent_version: target.version,
                    title: prompt.slice(0, 80),
                    description: prompt,
                  },
                );
            })
          }
        >
          {ja ? "作業を開始" : "Start work"}
        </Button>
        {area && session.data?.active_run_id && (
          <>
            <Button
              variant="outline"
              type="button"
              disabled={busy || !prompt.trim() || !steerReadsSelection}
              onClick={() =>
                void act(async () => {
                  const expectedRunId = session.data!.active_run_id!;
                  const pages = selectedPages.filter((page) =>
                    steered.includes(page),
                  );
                  const signature = JSON.stringify([
                    area.id,
                    expectedRunId,
                    prompt,
                    pages,
                  ]);
                  if (pendingSteer.current?.signature !== signature)
                    pendingSteer.current = {
                      signature,
                      key: crypto.randomUUID(),
                    };
                  await post(`/working-areas/${area.id}/steer`, {
                    idempotency_key: pendingSteer.current.key,
                    expected_run_id: expectedRunId,
                    content: prompt,
                    ...(pages.length ? { read_pages: pages } : {}),
                  });
                  pendingSteer.current = undefined;
                  setPrompt("");
                  setRequested(pages);
                  setReadPages([]);
                })
              }
            >
              {ja ? "実行中の Run に指示" : "Steer active Run"}
            </Button>
            <Button
              variant="outline"
              type="button"
              onClick={() =>
                void act(() =>
                  post(`/executions/${session.data!.active_run_id}/control`, {
                    action: "cancel",
                  }),
                )
              }
            >
              {ja ? "Run を停止" : "Stop Run"}
            </Button>
          </>
        )}
      </div>
      {session.isError && <Alert>{session.error.message}</Alert>}
      {area && active && <LivePreviewPanel run={active} />}
      {area && active && (
        <ScratchOperations
          key={`${area.id}:${active}`}
          area={area}
          run={active}
          refresh={refresh}
          inlineIds={inlineIds}
        />
      )}
      <ThreadScratchActions thread={thread} onDeleted={onDeleted} />
      <ProviderToolActivity
        context={shownRun.isError ? undefined : shownRun.data?.context}
        journal={
          shownRun.isError ? undefined : shownRun.data?.provider_tool_activity
        }
      />
      {error && <Alert>{error}</Alert>}
    </section>
  );
}

function ThreadScratchActions({
  thread,
  onDeleted,
}: {
  thread: string;
  onDeleted: () => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [clearing, setClearing] = useState(false);
  const deleteKey = useRef<string | undefined>(undefined);
  const clear = async () => {
    if (
      !window.confirm(
        ja
          ? "このスレッドの作業フォルダーを消去しますか？"
          : "Clear this Thread’s Scratch folder?",
      )
    )
      return;
    setBusy(true);
    setError("");
    try {
      await post(`/threads/${thread}/scratch/clear`);
      setClearing(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    if (
      !window.confirm(
        ja
          ? "このスレッドと作業フォルダーを削除しますか？"
          : "Delete this Thread and its Scratch folder?",
      )
    )
      return;
    setBusy(true);
    setError("");
    try {
      deleteKey.current ??= crypto.randomUUID();
      await post(`/threads/${thread}/delete`, {
        idempotency_key: deleteKey.current,
      });
      onDeleted();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <details className={panelClass}>
      <summary>{ja ? "スレッドの管理" : "Manage Thread"}</summary>
      <div className={inlineFormClass}>
        <Button
          variant="outline"
          type="button"
          disabled={busy}
          onClick={() => void clear()}
        >
          {ja ? "作業フォルダーを消去" : "Clear Scratch folder"}
        </Button>
        <Button
          variant="outline"
          type="button"
          disabled={busy}
          onClick={() => void remove()}
        >
          {ja ? "スレッドを削除" : "Delete Thread"}
        </Button>
      </div>
      {clearing && (
        <Hint role="status">
          {ja ? "消去をリクエストしました。" : "Folder wipe requested."}
        </Hint>
      )}
      {error && <Alert>{error}</Alert>}
    </details>
  );
}
