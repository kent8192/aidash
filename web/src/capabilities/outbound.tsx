import { useRef, useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { Field, useI18n } from "../ui";
import { apiFetch } from "../transport";
import { post, saveFile, type CoreFile } from "./client";
type FetchResult = {
  operation_id: string;
  status: string;
  url: string;
  final_url?: string;
  output_file?: CoreFile;
  http_status?: number;
  error?: { message: string };
};
export function OutboundFiles({
  run,
  area,
  revision,
}: {
  run: string;
  area: string;
  revision: number;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [url, setUrl] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const request = useRef<{ url: string; key: string } | undefined>(undefined);
  const query = useInfiniteQuery({
    queryKey: ["core-outbound", run],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{ items: FetchResult[]; next_cursor: string | null }>(
        `/api/runs/${run}/outbound${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await action();
      await query.refetch();
      await client.invalidateQueries({
        queryKey: ["core-operation-history", run],
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const filename = (item: FetchResult) => {
    try {
      return (
        new URL(item.final_url ?? item.url).pathname.split("/").at(-1) ?? ""
      );
    } catch {
      return "";
    }
  };
  return (
    <details className="core-panel">
      <summary>
        {ja
          ? "外部ファイルと Python パッケージ"
          : "External files and Python packages"}
      </summary>
      <Field label="HTTPS URL">
        <input
          type="url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
      </Field>
      <button
        type="button"
        disabled={busy || !url}
        onClick={() =>
          void act(async () => {
            if (request.current?.url !== url)
              request.current = { url, key: crypto.randomUUID() };
            await post(`/runs/${run}/outbound`, {
              url,
              idempotency_key: request.current.key,
            });
          })
        }
      >
        {ja ? "取得を要求" : "Request fetch"}
      </button>
      {items.map((item) => (
        <article key={item.operation_id}>
          <strong>{item.url}</strong>
          <p role="status">
            {item.status}
            {item.http_status ? ` · HTTP ${item.http_status}` : ""}
          </p>
          {item.error && <p role="alert">{item.error.message}</p>}
          {item.output_file && (
            <>
              <small>SHA-256: {item.output_file.digest}</small>
              <div className="core-inline">
                <button
                  type="button"
                  onClick={() =>
                    void act(() =>
                      saveFile(
                        `/working-areas/${area}/files/${item.output_file!.file_id}`,
                      ),
                    )
                  }
                >
                  {ja ? "ダウンロード" : "Download"}
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() =>
                    void act(() =>
                      post(`/runs/${run}/files/materialize`, {
                        idempotency_key: crypto.randomUUID(),
                        expected_revision: revision,
                        path:
                          filename(item) || `download-${item.operation_id}.bin`,
                        source: {
                          kind: "file",
                          file_id: item.output_file!.file_id,
                          expected_digest: item.output_file!.digest,
                        },
                      }),
                    )
                  }
                >
                  {ja ? "作業ファイルへコピー" : "Copy to working files"}
                </button>
              </div>
              {item.status === "completed" &&
                item.http_status === 200 &&
                filename(item).endsWith(".whl") && (
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={selected.includes(item.operation_id)}
                      onChange={(e) =>
                        setSelected(
                          e.target.checked
                            ? [...selected, item.operation_id]
                            : selected.filter((id) => id !== item.operation_id),
                        )
                      }
                    />
                    {ja
                      ? "Python に導入する wheel"
                      : "Wheel to install in Python"}
                  </label>
                )}
            </>
          )}
        </article>
      ))}
      {selected.length > 0 && (
        <>
          <p>
            {ja
              ? "選択した wheel で追加パッケージ一式を置き換えます。必要な依存 wheel も選択してください。Python のメモリはリセットされ、次の実行前に確認が必要です。"
              : "Replace the package overlay with these wheels. Select required dependency wheels too. Python memory resets and requires acknowledgement before the next execution."}
          </p>
          <button
            type="button"
            disabled={busy || selected.length > 16}
            onClick={() =>
              void act(() =>
                post(`/runs/${run}/python/install`, {
                  idempotency_key: crypto.randomUUID(),
                  expected_revision: revision,
                  wheels: items
                    .filter((i) => selected.includes(i.operation_id))
                    .map((i) => ({
                      outbound_operation_id: i.operation_id,
                      filename: filename(i),
                      sha256: i.output_file!.digest,
                    })),
                }),
              )
            }
          >
            {ja ? "選択したパッケージを導入" : "Install selected packages"}
          </button>
        </>
      )}
      {query.hasNextPage && (
        <button type="button" onClick={() => void query.fetchNextPage()}>
          {ja ? "さらに表示" : "Load more"}
        </button>
      )}
      {(error || query.isError) && (
        <p role="alert">{error || query.error?.message}</p>
      )}
    </details>
  );
}
