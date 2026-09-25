import { useRef, useState } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import type { State } from "../types";
import {
  post,
  saveFile,
  type Area,
  type CoreFile,
  type Operation,
} from "./client";
import { CapabilityApprovals } from "./approvals";
import "./style.css";
import { OutboundFiles } from "./outbound";
import { SkillFiles } from "./skills";
import { DisplayFile } from "./display";
import { TransferHistory } from "./transfers";
type Session = {
  active_run_id: string | null;
  last_run_id: string | null;
  last_agent_version: string | null;
  queue: { run_id: string; phase: string; control: string; sequence: number }[];
};
type Recipient = {
  node_id: string;
  agent_id: string;
  agent_version: string;
  thread_id: string;
};
function OperationCard({
  run,
  operation,
  onSession,
}: {
  run: string;
  operation: Operation;
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
        `/runs/${run}/${["code_interpreter", "python_install"].includes(operation.kind ?? "") ? "python" : operation.kind}/poll`,
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
      {value.session_id && <small>Python session: {value.session_id}</small>}
      {value.output && <pre>{value.output}</pre>}
      {value.displays?.map((file) => (
        <DisplayFile
          key={file.file_id}
          area={value.area_id ?? ""}
          file={file}
        />
      ))}
      {(query.error || error || value.error) && (
        <p role="alert">
          {query.error?.message || error || value.error?.message}
        </p>
      )}
      {value.effects_may_have_occurred && (
        <p className="core-warning">
          {ja
            ? "既に行われた変更や外部処理は取り消されていない可能性があります。"
            : "Changes or external effects may already have occurred."}
        </p>
      )}
      <div className="core-inline">
        {value.next_offset !== null && value.next_offset !== undefined && (
          <button type="button" onClick={() => setOffset(value.next_offset!)}>
            {ja ? "出力の続き" : "More output"}
          </button>
        )}
        {["prepared", "submitted", "running", "cancelling"].includes(
          value.status,
        ) && (
          <button
            type="button"
            onClick={async () => {
              try {
                await post(
                  `/runs/${run}/${["code_interpreter", "python_install"].includes(operation.kind ?? "") ? "python" : operation.kind}/cancel`,
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
          </button>
        )}
        {value.session_id && (
          <button type="button" onClick={() => onSession(value.session_id!)}>
            {ja ? "この Python セッションを使用" : "Use this Python session"}
          </button>
        )}
      </div>
      {value.termination_confirmed && (
        <small>
          {ja ? "プロセスの停止を確認済み" : "Process termination confirmed"}
        </small>
      )}
      {value.writer_frozen && (
        <small>
          {ja
            ? "Python の状態を保存して待機中"
            : "Python state retained; execution frozen"}
        </small>
      )}
    </article>
  );
}
function FileOperations({
  area,
  run,
  data,
  refresh,
}: {
  area: Area;
  run: string;
  data: State;
  refresh: () => Promise<unknown>;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [read, setRead] = useState<{
    content?: string;
    next_offset?: number;
    file?: CoreFile;
  }>();
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState("working");
  const [mode, setMode] = useState("literal");
  const [search, setSearch] = useState<{
    matches?: { path: string; location: { line: number }; snippet: string }[];
    unavailable?: { file_id: string; error: string }[];
    next_cursor?: string;
  }>();
  const [code, setCode] = useState("");
  const [kind, setKind] = useState("python");
  const [session, setSession] = useState<string>();
  const [reset, setReset] = useState<Operation>();
  const [patch, setPatch] = useState("");
  const [patchPreview, setPatchPreview] = useState<{
    revision: number;
    preconditions: Record<string, string | null>;
  }>();
  const [shareNode, setShareNode] = useState(data.node.id);
  const [recipient, setRecipient] = useState("");
  const [transfer, setTransfer] = useState<Operation>();
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
        `/api/runs/${run}/core-operations${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const recipients = useInfiniteQuery({
    queryKey: ["core-recipients", shareNode],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{
        items: (Recipient & { workspace_id?: string })[];
        next_cursor: string | null;
      }>(
        `/api/file-recipients?node_id=${encodeURIComponent(shareNode)}${pageParam ? `&cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: selected.length > 0,
    retry: false,
  });
  const transferQuery = useQuery({
    queryKey: ["core-transfer", transfer?.operation_id],
    queryFn: () =>
      apiFetch<Operation>(`/api/file-transfers/${transfer!.operation_id}`),
    enabled: !!transfer && !transfer.receipt && shareNode !== data.node.id,
    refetchInterval: (q) =>
      ["pending", "transferring", "committing"].includes(
        q.state.data?.status ?? transfer?.status ?? "",
      )
        ? 1500
        : false,
    retry: false,
  });
  const files = area.manifest;
  const readFile = async (file: CoreFile, offset = 0) => {
    const result = await post<{ content: string; next_offset?: number }>(
      `/runs/${run}/files/read`,
      {
        file_id: file.file_id,
        representation: "text",
        expected_digest: file.digest,
        offset,
      },
    );
    setRead({ ...result, file });
  };
  const runCode = async (expected_session_id?: string) => {
    const result = await mutate<Operation>(`/runs/${run}/${kind}`, {
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
      <details className="core-panel" open>
        <summary>
          {ja ? "作業ファイル" : "Working files"} · {area.revision}
        </summary>
        <div className="core-inline">
          <select
            aria-label={ja ? "検索対象" : "Search scope"}
            value={scope}
            onChange={(e) => setScope(e.target.value)}
          >
            {["working", "references", "received"].map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
          <select
            aria-label={ja ? "検索方法" : "Search mode"}
            value={mode}
            onChange={(e) => setMode(e.target.value)}
          >
            {["literal", "regex", "path"].map((v) => (
              <option key={v}>{v}</option>
            ))}
          </select>
          <input
            aria-label={ja ? "検索語" : "Search query"}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void act(async () =>
                setSearch(
                  await post(`/runs/${run}/files/search`, {
                    query,
                    mode,
                    scope,
                  }),
                ),
              )
            }
          >
            {ja ? "検索" : "Search"}
          </button>
        </div>
        {search && (
          <div>
            {search.matches?.map((m, i) => (
              <p key={i}>
                {m.path}:{m.location.line} {m.snippet}
              </p>
            ))}
            {search.unavailable?.map((file) => (
              <p key={file.file_id}>
                {
                  area.manifest.find((entry) => entry.file_id === file.file_id)
                    ?.path
                }
                {ja
                  ? "：テキストとして検索できません。原本をダウンロードしてください。"
                  : ": Text search unavailable. Download the original file."}
              </p>
            ))}
            {search.next_cursor && (
              <button
                type="button"
                onClick={() =>
                  void act(async () =>
                    setSearch(
                      await post(`/runs/${run}/files/search`, {
                        query,
                        mode,
                        scope,
                        cursor: search.next_cursor,
                      }),
                    ),
                  )
                }
              >
                {ja ? "検索の続き" : "More matches"}
              </button>
            )}
          </div>
        )}
        <div className="core-files">
          <table>
            <thead>
              <tr>
                <th>{ja ? "選択" : "Select"}</th>
                <th>{ja ? "ファイル" : "File"}</th>
                <th>{ja ? "操作" : "Actions"}</th>
              </tr>
            </thead>
            <tbody>
              {files.map((f) => (
                <tr key={f.file_id}>
                  <td>
                    <input
                      aria-label={`${ja ? "共有対象" : "Share"}: ${f.path}`}
                      type="checkbox"
                      checked={selected.includes(f.file_id)}
                      onChange={(e) =>
                        setSelected((ids) =>
                          e.target.checked
                            ? [...ids, f.file_id]
                            : ids.filter((id) => id !== f.file_id),
                        )
                      }
                    />
                  </td>
                  <td>
                    {f.path}
                    <small>
                      {f.scope} · {f.size} bytes
                    </small>
                  </td>
                  <td>
                    <button
                      type="button"
                      onClick={() => void act(() => readFile(f))}
                    >
                      {ja ? "読む" : "Read"}
                    </button>
                    <button
                      type="button"
                      onClick={() =>
                        void act(() =>
                          saveFile(
                            `/working-areas/${area.id}/files/${f.file_id}/download`,
                          ),
                        )
                      }
                    >
                      {ja ? "保存" : "Download"}
                    </button>
                    {f.scope !== "working" && (
                      <button
                        type="button"
                        onClick={() =>
                          void act(() =>
                            mutate(`/runs/${run}/files/materialize`, {
                              expected_revision: area.revision,
                              path: `copies/${f.path}`,
                              source: {
                                kind: "file",
                                file_id: f.file_id,
                                expected_digest: f.digest,
                              },
                            }),
                          )
                        }
                      >
                        {ja ? "作業用にコピー" : "Copy to working files"}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {read && (
          <>
            <pre>{read.content}</pre>
            {read.next_offset != null && read.file && (
              <button
                type="button"
                onClick={() =>
                  void act(() => readFile(read.file!, read.next_offset))
                }
              >
                {ja ? "本文の続き" : "Read more"}
              </button>
            )}
          </>
        )}
        {selected.length > 0 && (
          <fieldset>
            <legend>
              {ja ? "選択ファイルを共有" : "Share selected files"}
            </legend>
            <Field label={ja ? "ノード" : "Node"}>
              <select
                value={shareNode}
                onChange={(e) => {
                  setShareNode(e.target.value);
                  setRecipient("");
                }}
              >
                <option value={data.node.id}>
                  {ja ? "このノード" : "This node"}
                </option>
                {data.peers
                  .filter((p) => p.enabled)
                  .map((p) => (
                    <option key={p.node_id} value={p.node_id}>
                      {p.node_id}
                    </option>
                  ))}
              </select>
            </Field>
            {recipients.isError && (
              <p role="alert">{recipients.error.message}</p>
            )}
            <Field
              label={
                ja
                  ? "受信 Agent・バージョン・スレッド"
                  : "Recipient Agent, version and thread"
              }
            >
              <select
                value={recipient}
                onChange={(e) => setRecipient(e.target.value)}
              >
                <option value="">
                  {ja ? "受信先を選択" : "Choose a recipient"}
                </option>
                {recipients.data?.pages
                  .flatMap((p) => p.items)
                  .filter(
                    (r) =>
                      shareNode !== data.node.id ||
                      (r.workspace_id === area.workspace_id &&
                        (r.agent_id !== area.agent_id ||
                          r.thread_id !== area.thread_id)),
                  )
                  .map((r, i) => (
                    <option
                      key={i}
                      value={JSON.stringify({
                        node_id: r.node_id,
                        agent_id: r.agent_id,
                        agent_version: r.agent_version,
                        thread_id: r.thread_id,
                      })}
                    >
                      {r.agent_id}@{r.agent_version} · {r.thread_id}
                    </option>
                  ))}
              </select>
            </Field>
            {recipients.hasNextPage && (
              <button
                type="button"
                onClick={() => void recipients.fetchNextPage()}
              >
                {ja ? "共有先をさらに表示" : "More recipients"}
              </button>
            )}
            <button
              type="button"
              disabled={!recipient || busy}
              onClick={() =>
                void act(async () =>
                  setTransfer(
                    await mutate(`/runs/${run}/files/share`, {
                      expected_revision: area.revision,
                      files: files
                        .filter((f) => selected.includes(f.file_id))
                        .map((f) => ({
                          file_id: f.file_id,
                          expected_digest: f.digest,
                        })),
                      recipient: JSON.parse(recipient),
                    }),
                  ),
                )
              }
            >
              {ja ? "この内容を送信" : "Send this snapshot"}
            </button>
          </fieldset>
        )}
        {transfer && (
          <article>
            <p role="status">
              {ja ? "転送" : "Transfer"}:{" "}
              {transferQuery.data?.status ?? transfer.status}
            </p>
            {(transferQuery.data?.receipt ?? transfer.receipt) ? (
              <p>
                {ja
                  ? "受信先での保存を確認しました。"
                  : "Recipient storage confirmed."}
              </p>
            ) : (
              <p>
                {ja ? "受領確認はまだありません。" : "No durable receipt yet."}
              </p>
            )}
            {transferQuery.isError && (
              <p role="alert">{transferQuery.error.message}</p>
            )}
            {!transfer.receipt && shareNode !== data.node.id && (
              <button
                type="button"
                onClick={() =>
                  void act(async () => {
                    setTransfer(
                      await post(
                        `/file-transfers/${transfer.operation_id}/reconcile`,
                      ),
                    );
                    await transferQuery.refetch();
                  })
                }
              >
                {ja ? "同じ転送を照会・再開" : "Reconcile this transfer"}
              </button>
            )}
          </article>
        )}
      </details>
      <details className="core-panel">
        <summary>Shell · Python</summary>
        <Field label={ja ? "実行する言語" : "Runtime"}>
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            <option value="python">Python</option>
            <option value="shell">Shell</option>
          </select>
        </Field>
        <Field label={ja ? "コード" : "Code"}>
          <textarea
            value={code}
            onChange={(e) => setCode(e.target.value)}
            spellCheck={false}
          />
        </Field>
        <button
          type="button"
          disabled={busy || !code.trim()}
          onClick={() => void act(() => runCode())}
        >
          {ja ? "実行" : "Execute"}
        </button>
        {reset && (
          <article>
            <p className="core-warning">
              {ja
                ? "Python のメモリはリセットされました。変数は復元されません。保存済みファイルを確認してから再実行してください。"
                : "Python memory was reset. Variables were not restored. Review the saved files before running again."}
            </p>
            <small>{reset.reset_reason}</small>
            <button
              type="button"
              disabled={busy}
              onClick={() => void act(() => runCode(reset.session_id))}
            >
              {ja
                ? "リセットを確認してこのコードを実行"
                : "Acknowledge reset and execute this code"}
            </button>
          </article>
        )}
        {history.isError && <p role="alert">{history.error.message}</p>}
        {history.data?.pages
          .flatMap((p) => p.items)
          .map((op) => (
            <OperationCard
              key={op.operation_id}
              run={run}
              operation={op}
              onSession={setSession}
            />
          ))}
        {history.hasNextPage && (
          <button type="button" onClick={() => void history.fetchNextPage()}>
            {ja ? "さらに表示" : "Load more"}
          </button>
        )}
      </details>
      <details className="core-panel">
        <summary>
          {ja ? "パッチの確認と適用" : "Review and apply a patch"}
        </summary>
        <Field label="Patch">
          <textarea
            value={patch}
            onChange={(e) => {
              setPatch(e.target.value);
              setPatchPreview(undefined);
            }}
            spellCheck={false}
          />
        </Field>
        <button
          type="button"
          disabled={!patch.trim()}
          onClick={() => {
            const paths = [
              ...patch.matchAll(/^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm),
            ].map((m) => m[1]);
            setPatchPreview({
              revision: area.revision,
              preconditions: Object.fromEntries(
                paths.map((path) => [
                  path,
                  files.find((f) => f.scope === "working" && f.path === path)
                    ?.digest ?? null,
                ]),
              ),
            });
          }}
        >
          {ja ? "対象と変更を確認" : "Review affected files"}
        </button>
        {patchPreview && (
          <>
            <p>
              {ja ? "適用対象の版" : "Expected revision"}:{" "}
              {patchPreview.revision}
            </p>
            <pre>{patch}</pre>
            <ul>
              {Object.entries(patchPreview.preconditions).map(
                ([path, digest]) => (
                  <li key={path}>
                    {path}
                    <small>
                      {digest ?? (ja ? "新規ファイル" : "New file")}
                    </small>
                  </li>
                ),
              )}
            </ul>
            <button
              type="button"
              disabled={busy || patchPreview.revision !== area.revision}
              onClick={() =>
                void act(() =>
                  mutate(`/runs/${run}/patch`, {
                    expected_revision: patchPreview.revision,
                    preconditions: patchPreview.preconditions,
                    patch,
                  }),
                )
              }
            >
              {ja ? "この変更を適用" : "Apply these changes"}
            </button>
          </>
        )}
      </details>
      <SkillFiles run={run} />
      <TransferHistory area={area.id} />
      <OutboundFiles run={run} area={area.id} revision={area.revision} />
      <CapabilityApprovals area={area.id} />
      {error && <p role="alert">{error}</p>}
    </>
  );
}
export function ThreadCapabilities({
  workspace,
  thread,
  data,
  onDeleted,
}: {
  workspace: string;
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
  const query = useQuery({
    queryKey: ["core-areas", workspace, thread],
    queryFn: ({ signal }) =>
      apiFetch<{ items: Area[] }>(
        `/api/working-areas?workspace_id=${workspace}&thread_id=${thread}`,
        { signal },
      ),
    refetchInterval: 2000,
    retry: false,
  });
  const area =
    query.data?.items.find((a) => a.id === selected) ?? query.data?.items[0];
  const session = useQuery({
    queryKey: ["core-session", area?.id],
    queryFn: ({ signal }) =>
      apiFetch<Session>(`/api/working-areas/${area!.id}/session`, { signal }),
    enabled: !!area,
    refetchInterval: 2000,
    retry: false,
  });
  const active = session.data?.active_run_id ?? session.data?.last_run_id;
  const agents = data.registry.filter(
    (e) =>
      e.kind === "agent" &&
      e.config &&
      Object.values(
        (e.config.core_capabilities ?? {}) as Record<string, unknown>,
      ).some(Boolean),
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
  const availableAgents = agents.filter((e) => !area || e.id === area.agent_id);
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
  if (!area && agents.length === 0 && !query.isError) return null;
  return (
    <section className="core-panel">
      <h3>{ja ? "Agent の作業" : "Agent work"}</h3>
      {query.isError && <p role="alert">{query.error.message}</p>}
      {
        <Field label={ja ? "実行する Agent" : "Agent to run"}>
          <select
            value={effectiveAgent}
            onChange={(e) => setAgent(e.target.value)}
          >
            <option value="">{ja ? "選択" : "Choose"}</option>
            {availableAgents.map((e) => (
              <option
                key={`${e.id}@${e.version}`}
              >{`${e.id}@${e.version}`}</option>
            ))}
          </select>
        </Field>
      }
      {(query.data?.items.length ?? 0) > 1 && (
        <Field label={ja ? "作業領域" : "Working area"}>
          <select
            value={area?.id}
            onChange={(e) => setSelected(e.target.value)}
          >
            {query.data?.items.map((a) => (
              <option key={a.id} value={a.id}>
                {a.agent_id}
              </option>
            ))}
          </select>
        </Field>
      )}
      <Field label={ja ? "次の指示" : "Next instruction"}>
        <textarea
          disabled={busy}
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
        />
      </Field>
      <div className="core-inline">
        <button
          type="button"
          disabled={busy || !prompt.trim() || !target}
          onClick={() =>
            void act(async () => {
              if (area) {
                const version =
                  target?.id === area.agent_id ? target.version : undefined;
                if (!version)
                  throw new Error("Select an available Agent version");
                await post(`/working-areas/${area.id}/queue`, {
                  idempotency_key: crypto.randomUUID(),
                  agent_version: version,
                  title: prompt.slice(0, 80),
                  description: prompt,
                });
              } else if (target)
                await post(
                  `/workspaces/${workspace}/threads/${thread}/agents/${encodeURIComponent(target.id)}/runs`,
                  {
                    idempotency_key: crypto.randomUUID(),
                    agent_version: target.version,
                    title: prompt.slice(0, 80),
                    description: prompt,
                  },
                );
              setPrompt("");
            })
          }
        >
          {area
            ? ja
              ? "次の Run に送る"
              : "Queue next Run"
            : ja
              ? "作業を開始"
              : "Start work"}
        </button>
        {area && session.data?.active_run_id && (
          <>
            <button
              type="button"
              disabled={busy || !prompt.trim()}
              onClick={() =>
                void act(async () => {
                  await post(`/working-areas/${area.id}/steer`, {
                    idempotency_key: crypto.randomUUID(),
                    expected_run_id: session.data!.active_run_id,
                    content: prompt,
                  });
                  setPrompt("");
                })
              }
            >
              {ja ? "実行中の Run に指示" : "Steer active Run"}
            </button>
            <button
              type="button"
              onClick={() =>
                void act(() =>
                  post(`/runs/${session.data!.active_run_id}/control`, {
                    action: "cancel",
                  }),
                )
              }
            >
              {ja ? "Run を停止" : "Stop Run"}
            </button>
          </>
        )}
      </div>
      {session.isError && <p role="alert">{session.error.message}</p>}
      {session.data && (
        <ol>
          {session.data.queue.map((r) => (
            <li key={r.run_id}>
              #{r.sequence} · {r.phase} · {r.control}
              {r.run_id === session.data.active_run_id
                ? ` · ${ja ? "実行対象" : "Active"}`
                : ""}
            </li>
          ))}
        </ol>
      )}
      {area && active && (
        <FileOperations
          key={`${area.id}:${active}`}
          area={area}
          run={active}
          data={data}
          refresh={refresh}
        />
      )}
      <ThreadDeletion
        workspace={workspace}
        thread={thread}
        areas={query.data?.items ?? []}
        onDeleted={onDeleted}
      />
      {session.data?.last_run_id &&
        !session.data?.active_run_id &&
        area?.state === "active" && (
          <p className="core-warning">
            {ja
              ? "作業が終了しました。ファイルは保持されています。設定で復元可能な整理を選べます。"
              : "Work has ended. Files are retained; recoverable cleanup is available in settings."}{" "}
            <a href="/settings?view=workingFiles">
              {ja ? "作業ファイルの設定" : "Working file settings"}
            </a>
          </p>
        )}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}

function ThreadDeletion({
  workspace,
  thread,
  areas,
  onDeleted,
}: {
  workspace: string;
  thread: string;
  areas: Area[];
  onDeleted: () => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [choice, setChoice] = useState("keep");
  const [confirmations, setConfirmations] =
    useState<Record<string, { confirmation_id: string; revision: number }>>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const confirmed =
    areas.length > 0 &&
    areas.every((a) => confirmations?.[a.id]?.revision === a.revision);
  return (
    <details className="core-panel">
      <summary>{ja ? "このスレッドを削除" : "Delete this thread"}</summary>
      <p>
        {ja
          ? "スレッドが消える前に、作業ファイルの扱いを選んでください。残したファイルは設定から管理できます。"
          : "Choose what happens to working files before the thread disappears. Retained files remain in settings."}
      </p>
      <ul>
        {areas.map((a) => (
          <li key={a.id}>
            {a.agent_id} ·{" "}
            {a.manifest.filter((f) => f.scope !== "references").length}{" "}
            {ja ? "ファイル" : "files"} ·{" "}
            {a.manifest
              .filter((f) => f.scope !== "references")
              .reduce((n, f) => n + f.size, 0)}{" "}
            bytes
          </li>
        ))}
      </ul>
      <Field label={ja ? "ファイルの扱い" : "File retention"}>
        <select
          value={choice}
          onChange={(e) => {
            setChoice(e.target.value);
            setConfirmations(undefined);
          }}
        >
          <option value="keep">{ja ? "ファイルを残す" : "Keep files"}</option>
          <option value="recoverable">
            {ja ? "復元可能な整理" : "Recoverable cleanup"}
          </option>
          <option value="irreversible">
            {ja ? "復元できない削除" : "Irreversible deletion"}
          </option>
        </select>
      </Field>
      {choice === "irreversible" && !confirmed && areas.length > 0 ? (
        <button
          type="button"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError("");
            try {
              const values: NonNullable<typeof confirmations> = {};
              for (const area of areas)
                values[area.id] = await post(
                  `/working-areas/${area.id}/deletion-confirmation`,
                  { expected_revision: area.revision },
                );
              setConfirmations(values);
            } catch (e) {
              setError(String(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          {ja ? "完全削除の対象を確認" : "Review irreversible deletion"}
        </button>
      ) : (
        <>
          <p className="core-warning">
            {choice === "irreversible"
              ? ja
                ? "上記の作業ファイルは復元できなくなります。"
                : "The working files above will no longer be recoverable."
              : ja
                ? "このスレッドを閉じて削除します。"
                : "This thread will be closed and deleted."}
          </p>
          <button
            type="button"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              setError("");
              try {
                await post(
                  `/workspaces/${workspace}/threads/${thread}/delete`,
                  {
                    idempotency_key: crypto.randomUUID(),
                    files: areas.map((a) => ({
                      area_id: a.id,
                      expected_revision: a.revision,
                      choice,
                      confirmation_id:
                        choice === "irreversible"
                          ? confirmations?.[a.id]?.confirmation_id
                          : null,
                    })),
                  },
                );
                onDeleted();
              } catch (e) {
                setError(String(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {ja
              ? "この選択でスレッドを削除"
              : "Delete thread with these choices"}
          </button>
        </>
      )}
      {error && <p role="alert">{error}</p>}
    </details>
  );
}
