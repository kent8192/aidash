import { useInfiniteQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiFetch } from "../transport";
import { useI18n } from "../ui";
import { post, type Operation } from "./client";
type Transfer = Operation & {
  recipient: {
    node_id: string;
    agent_id: string;
    agent_version: string;
    thread_id: string;
  };
  manifest_digest: string;
};
export function TransferHistory({ area }: { area: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [error, setError] = useState("");
  const query = useInfiniteQuery({
    queryKey: ["core-transfers", area],
    initialPageParam: "",
    retry: false,
    refetchInterval: 2000,
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{ items: Transfer[]; next_cursor: string | null }>(
        `/api/working-areas/${area}/transfers${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
  return (
    <details className="core-panel">
      <summary>
        {ja ? "共有履歴と受領結果" : "Transfer history and receipts"}
      </summary>
      {query.data?.pages
        .flatMap((p) => p.items)
        .map((item) => (
          <article key={item.operation_id}>
            <strong>
              {item.recipient.agent_id}@{item.recipient.agent_version} ·{" "}
              {item.status}
            </strong>
            <p>
              {item.recipient.node_id} · {item.recipient.thread_id}
            </p>
            <small>
              {ja ? "転送番号" : "Transfer"}: {item.operation_id}
            </small>
            <small>SHA-256: {item.manifest_digest}</small>
            {item.receipt && (
              <p>
                {ja
                  ? "受領を確認しました。相手側の独立したコピーとして保持されます。"
                  : "Receipt confirmed. The recipient owns an independent copy."}
              </p>
            )}
            {item.effects_may_have_occurred && !item.receipt && (
              <p className="core-warning">
                {ja
                  ? "受領結果がまだ確認できません。同じ転送を照会してください。"
                  : "Delivery may have occurred. Reconcile this transfer to confirm its receipt."}
              </p>
            )}
            {item.error && <p role="alert">{item.error.message}</p>}
            {item.status === "uncertain" && (
              <button
                type="button"
                onClick={async () => {
                  try {
                    await post(
                      `/file-transfers/${item.operation_id}/reconcile`,
                    );
                    await query.refetch();
                  } catch (e) {
                    setError(String(e));
                  }
                }}
              >
                {ja ? "同じ転送を照会・再開" : "Reconcile this transfer"}
              </button>
            )}
          </article>
        ))}
      {query.hasNextPage && (
        <button type="button" onClick={() => void query.fetchNextPage()}>
          {ja ? "さらに表示" : "Load more"}
        </button>
      )}
      {(error || query.error) && (
        <p role="alert">{error || query.error?.message}</p>
      )}
    </details>
  );
}
