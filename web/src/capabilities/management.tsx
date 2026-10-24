import { Input } from "../components/ui/input";
import { panelClass } from "./display";
import {
  Alert,
  Check,
  Group,
  Hint,
  Loading,
  Notice,
} from "../components/patterns";
import { NativeSelect } from "../components/ui/native-select";
import { Button } from "../components/ui/button";
import { useEffect, useId, useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import { post } from "./client";
import { OriginalReferences } from "./configuration";
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
      <Hint>
        {ja ? "スレッド" : "Thread"}: {area.thread_id} ·{" "}
        {ja ? "所有者" : "Owner"}: {area.owner}
      </Hint>
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
          <Button
            variant="outline"
            type="button"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                const restored = await post<{ thread: { id: string } }>(
                  `/workspaces/${area.workspace_id}/working-areas/${area.area_id}/restore/new-thread`,
                  {
                    idempotency_key: crypto.randomUUID(),
                    expected_revision: area.revision,
                    snapshot_id: area.snapshot_id,
                    content: ja
                      ? "保存済み作業ファイルの復元"
                      : "Restore retained working files",
                  },
                );
                setThread(restored.thread.id);
              })
            }
          >
            {ja ? "新しいスレッドへ復元" : "Restore into a new thread"}
          </Button>
          <Field label={ja ? "復元先スレッド ID" : "Destination thread ID"}>
            <Input value={thread} onChange={(e) => setThread(e.target.value)} />
          </Field>
          <Button
            variant="outline"
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
          </Button>
        </>
      ) : null}
      {["active", "retained", "recoverable"].includes(area.state) && (
        <Group
          nested
          legend={ja ? "作業ファイルの整理" : "Working file cleanup"}
        >
          <Field label={ja ? "整理方法" : "Retention choice"}>
            <NativeSelect
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
            </NativeSelect>
          </Field>
          {choice === "irreversible" &&
          (!confirmation || confirmation.revision !== area.revision) ? (
            <Button
              variant="outline"
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
            </Button>
          ) : (
            <>
              {choice === "irreversible" && (
                <Notice tone="warning">
                  {ja
                    ? `「${area.agent_id}」の ${area.files} ファイルを完全に削除します。復元用のコピーも残りません。`
                    : `Permanently delete ${area.files} files for ${area.agent_id}. No recovery copy will remain.`}
                </Notice>
              )}
              <Button
                variant="outline"
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
              </Button>
            </>
          )}
        </Group>
      )}
      {["cleaning", "cleanup_failed"].includes(area.state) && (
        <>
          <Hint role="status">
            {ja
              ? "削除処理を確認中です。完了するまで再利用できません。"
              : "Cleanup is being reconciled. Files remain unavailable until it completes."}
          </Hint>
          {area.cleanup_operation_id && (
            <Button
              variant="outline"
              type="button"
              disabled={busy}
              onClick={() =>
                void act(() =>
                  post(`/file-cleanups/${area.cleanup_operation_id}/reconcile`),
                )
              }
            >
              {ja ? "整理処理を再確認" : "Reconcile cleanup"}
            </Button>
          )}
        </>
      )}
      {error && <Alert>{error}</Alert>}
    </article>
  );
}
export function WorkingFileSettings({
  workspace,
}: { workspace?: string } = {}) {
  const headingId = useId();
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [allWorkspaces, setAllWorkspaces] = useState(false);
  const query = useInfiniteQuery({
    queryKey: ["core-managed"],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<Page>(
        `/api/working-files${pageParam ? `?cursor=${encodeURIComponent(pageParam)}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 3000,
    retry: false,
  });
  const areas = (query.data?.pages.flatMap((page) => page.items) ?? []).filter(
    (area) => allWorkspaces || !workspace || area.workspace_id === workspace,
  );
  const searching = Boolean(
    workspace && !allWorkspaces && areas.length === 0 && query.hasNextPage,
  );
  const { isFetching, isError, fetchNextPage } = query;
  useEffect(() => {
    if (searching && !isFetching && !isError) void fetchNextPage();
  }, [searching, isFetching, isError, fetchNextPage]);
  return (
    <>
      <section className={panelClass} aria-labelledby={headingId}>
        <h2 id={headingId}>{ja ? "作業ファイル" : "Working files"}</h2>
        <p>
          {ja
            ? "作業の完了やスレッドの削除だけではファイルは消えません。ここで保存、復元可能な整理、完全削除を選べます。"
            : "Files remain after work ends or a thread is deleted. Choose retention, recoverable cleanup or irreversible deletion here."}
        </p>
        {workspace && (
          <Check
            checked={allWorkspaces}
            onChange={(event) => setAllWorkspaces(event.target.checked)}
          >
            {ja
              ? "すべての依頼の作業ファイルを表示"
              : "Show working files from all requests"}
          </Check>
        )}
        {query.isError && <Alert>{query.error.message}</Alert>}
        {(query.isPending || (searching && !query.isError)) && (
          <Loading>{ja ? "読み込み中…" : "Loading…"}</Loading>
        )}
        {areas.map((area) => (
          <AreaLifecycle key={area.area_id} area={area} />
        ))}
        {query.hasNextPage && (
          <Button
            variant="outline"
            type="button"
            disabled={query.isFetchingNextPage}
            onClick={() => void query.fetchNextPage()}
          >
            {ja ? "さらに表示" : "Load more"}
          </Button>
        )}
      </section>
      <AgentCapabilities />
      <OriginalReferences />
    </>
  );
}
