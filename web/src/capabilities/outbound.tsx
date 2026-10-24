import { Input } from "../components/ui/input";
import { panelClass } from "./display";
import { Alert, Hint, inlineFormClass } from "../components/patterns";
import { Button } from "../components/ui/button";
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
    <details className={panelClass}>
      <summary>{ja ? "外部ファイル" : "External files"}</summary>
      <Field label="HTTPS URL">
        <Input
          type="url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
      </Field>
      <Button
        variant="outline"
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
            request.current = undefined;
          })
        }
      >
        {ja ? "取得を要求" : "Request fetch"}
      </Button>
      {items.map((item) => (
        <article key={item.operation_id}>
          <strong>{item.url}</strong>
          <Hint role="status">
            {item.status}
            {item.http_status ? ` · HTTP ${item.http_status}` : ""}
          </Hint>
          {item.error && <Alert>{item.error.message}</Alert>}
          {item.output_file && (
            <>
              <Hint>SHA-256: {item.output_file.digest}</Hint>
              <div className={inlineFormClass}>
                <Button
                  variant="outline"
                  type="button"
                  onClick={() =>
                    void act(() =>
                      saveFile(
                        `/working-areas/${area}/files/${item.output_file!.file_id}/download`,
                      ),
                    )
                  }
                >
                  {ja ? "ダウンロード" : "Download"}
                </Button>
                <Button
                  variant="outline"
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
                </Button>
              </div>
            </>
          )}
        </article>
      ))}
      {query.hasNextPage && (
        <Button
          variant="outline"
          type="button"
          onClick={() => void query.fetchNextPage()}
        >
          {ja ? "さらに表示" : "Load more"}
        </Button>
      )}
      {(error || query.isError) && (
        <Alert>{error || query.error?.message}</Alert>
      )}
    </details>
  );
}
