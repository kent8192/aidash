import { useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import { post } from "./client";
import { OriginalReferences } from "./configuration";
import "./style.css";
import { AgentCapabilities } from "./agent";
type Managed = {
  area_id: string;
  workspace_id: string;
  thread_id: string;
  agent_id: string;
  owner: string;
  state: string;
  revision: number;
  generation: number;
  files: number;
  bytes: number;
  snapshot_id: string | null;
  recovery_expires_at: string | null;
  cleanup_operation_id?: string | null;
};
type Page = { items: Managed[]; next_cursor: string | null };
export function AreaLifecycle({ area }: { area: Managed }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [choice, setChoice] = useState("keep");
  const [confirmation, setConfirmation] = useState<{
    confirmation_id: string;
    revision: number;
  }>();
  const [thread, setThread] = useState(area.thread_id);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const act = async (action: () => Promise<unknown>, clear = true) => {
    setBusy(true);
    setError("");
    try {
      await action();
      if (clear) setConfirmation(undefined);
      await client.invalidateQueries({ queryKey: ["core-managed"] });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <article>
      <h3>{area.agent_id}</h3>
      <p>
        {area.files} {ja ? "ファイル" : "files"} ·{" "}
        {(area.bytes / 1024).toFixed(1)} KiB · {area.state}
      </p>
      <small>
        {ja ? "スレッド" : "Thread"}: {area.thread_id} ·{" "}
        {ja ? "所有者" : "Owner"}: {area.owner}
      </small>
      {area.recovery_expires_at && (
        <p>
          {ja ? "復元期限" : "Recover until"}:{" "}
          <time dateTime={area.recovery_expires_at}>
            {new Date(area.recovery_expires_at).toLocaleString()}
          </time>
        </p>
      )}
      {["recoverable", "retained"].includes(area.state) && area.snapshot_id ? (
        <>
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                const created = await post<{ message: { id: string } }>(
                  `/workspaces/${area.workspace_id}/thread-messages`,
                  {
                    idempotency_key: crypto.randomUUID(),
                    content: ja
                      ? "保存済み作業ファイルの復元"
                      : "Restore retained working files",
                    thread_id: null,
                    attachment_ids: [],
                  },
                );
                const destination = await post<{ id: string }>(
                  `/workspaces/${area.workspace_id}/threads`,
                  { root_message_id: created.message.id },
                );
                await post(`/working-areas/${area.area_id}/restore`, {
                  idempotency_key: crypto.randomUUID(),
                  expected_revision: area.revision,
                  snapshot_id: area.snapshot_id,
                  thread_id: destination.id,
                });
                setThread(destination.id);
              })
            }
          >
            {ja ? "新しいスレッドへ復元" : "Restore into a new thread"}
          </button>
          <Field label={ja ? "復元先スレッド ID" : "Destination thread ID"}>
            <input value={thread} onChange={(e) => setThread(e.target.value)} />
          </Field>
          <button
            type="button"
            disabled={busy || !thread.trim()}
            onClick={() =>
              void act(() =>
                post(`/working-areas/${area.area_id}/restore`, {
                  idempotency_key: crypto.randomUUID(),
                  expected_revision: area.revision,
                  snapshot_id: area.snapshot_id,
                  thread_id: thread,
                }),
              )
            }
          >
            {ja ? "ファイルを復元" : "Restore files"}
          </button>
        </>
      ) : null}
      {["active", "retained", "recoverable"].includes(area.state) && (
        <fieldset>
          <legend>{ja ? "作業ファイルの整理" : "Working file cleanup"}</legend>
          <Field label={ja ? "整理方法" : "Retention choice"}>
            <select
              value={choice}
              onChange={(e) => {
                setChoice(e.target.value);
                setConfirmation(undefined);
              }}
            >
              <option value="keep">
                {ja ? "残す（初期選択）" : "Keep (default)"}
              </option>
              <option value="recoverable">
                {ja ? "復元可能な状態で整理" : "Clean up with recovery"}
              </option>
              <option value="irreversible">
                {ja ? "復元できない完全削除" : "Delete irreversibly"}
              </option>
            </select>
          </Field>
          {choice === "irreversible" &&
          (!confirmation || confirmation.revision !== area.revision) ? (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  const value = await post<{
                    confirmation_id: string;
                    revision: number;
                  }>(`/working-areas/${area.area_id}/deletion-confirmation`, {
                    expected_revision: area.revision,
                  });
                  setConfirmation(value);
                }, false)
              }
            >
              {ja ? "削除内容を確認" : "Review deletion"}
            </button>
          ) : (
            <>
              {choice === "irreversible" && (
                <p className="core-warning">
                  {ja
                    ? `「${area.agent_id}」の ${area.files} ファイルを完全に削除します。復元用のコピーも残りません。`
                    : `Permanently delete ${area.files} files for ${area.agent_id}. No recovery copy will remain.`}
                </p>
              )}
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void act(() =>
                    post(`/working-areas/${area.area_id}/cleanup`, {
                      idempotency_key: crypto.randomUUID(),
                      expected_revision: area.revision,
                      choice,
                      confirmation_id: confirmation?.confirmation_id ?? null,
                    }),
                  )
                }
              >
                {choice === "irreversible"
                  ? ja
                    ? "確認したファイルを完全削除"
                    : "Confirm irreversible deletion"
                  : ja
                    ? "この選択で整理"
                    : "Apply retention choice"}
              </button>
            </>
          )}
        </fieldset>
      )}
      {["cleaning", "cleanup_failed"].includes(area.state) && (
        <>
          <p role="status">
            {ja
              ? "削除処理を確認中です。完了するまで再利用できません。"
              : "Cleanup is being reconciled. Files remain unavailable until it completes."}
          </p>
          {area.cleanup_operation_id && (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void act(() =>
                  post(`/file-cleanups/${area.cleanup_operation_id}/reconcile`),
                )
              }
            >
              {ja ? "整理処理を再確認" : "Reconcile cleanup"}
            </button>
          )}
        </>
      )}
      {error && <p role="alert">{error}</p>}
    </article>
  );
}
export function WorkingFileSettings() {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const query = useInfiniteQuery({
    queryKey: ["core-managed"],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<Page>(
        `/api/working-files${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 3000,
    retry: false,
  });
  return (
    <>
      <section className="core-panel">
        <h2>{ja ? "作業ファイル" : "Working files"}</h2>
        <p>
          {ja
            ? "作業の完了やスレッドの削除だけではファイルは消えません。ここで保存、復元可能な整理、完全削除を選べます。"
            : "Files remain after work ends or a thread is deleted. Choose retention, recoverable cleanup or irreversible deletion here."}
        </p>
        {query.isError && <p role="alert">{query.error.message}</p>}
        {query.isPending && (
          <p role="status">{ja ? "読み込み中…" : "Loading…"}</p>
        )}
        {query.data?.pages
          .flatMap((p) => p.items)
          .map((area) => (
            <AreaLifecycle key={area.area_id} area={area} />
          ))}
        {query.hasNextPage && (
          <button type="button" onClick={() => void query.fetchNextPage()}>
            {ja ? "さらに表示" : "Load more"}
          </button>
        )}
      </section>
      <AgentCapabilities />
      <OriginalReferences />
    </>
  );
}
