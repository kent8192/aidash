import { Input } from "../components/ui/input";
import { panelClass } from "./display";
import {
  Alert,
  Facts,
  Hint,
  Loading,
  inlineFormClass,
} from "../components/patterns";
import { AgentBindings, type BindingConfiguration } from "../agent-bindings";
import { Button } from "../components/ui/button";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { Field, useI18n } from "../ui";
import { apiFetch } from "../transport";
import { post, saveFile } from "./client";
export type ReferenceBinding = { reference_id: string; digest: string };
export type CoreConfiguration = BindingConfiguration;
export const emptyCore: CoreConfiguration = {
  bindings: [],
  remove_default: [],
};
async function hash(bytes: ArrayBuffer) {
  return Array.from(
    new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
    (n) => n.toString(16).padStart(2, "0"),
  ).join("");
}
export function CapabilityConfiguration({
  value,
  change,
  node,
  entries,
  cluster = false,
}: {
  privateReferences?: boolean;
  value: CoreConfiguration;
  change: (v: CoreConfiguration) => void;
  node?: string;
  cluster?: boolean;
  entries?: Parameters<typeof AgentBindings>[0]["entries"];
}) {
  const query = useQuery({
    queryKey: ["binding-definitions"],
    queryFn: ({ signal }) =>
      apiFetch<NonNullable<Parameters<typeof AgentBindings>[0]["entries"]>>(
        "/api/registry",
        { signal },
      ),
    enabled: !entries,
    retry: false,
  });
  return (
    <AgentBindings
      value={value}
      change={change}
      entries={entries ?? query.data}
      node={node}
      cluster={cluster}
    />
  );
}
type Reference = ReferenceBinding & {
  name: string;
  state: string;
  revision: number;
  extraction_state?: string;
};
export function OriginalReferences({
  attached = [],
  onAttach,
  onDetach,
}: {
  attached?: ReferenceBinding[];
  onAttach?: (reference: ReferenceBinding) => void;
  onDetach?: (id: string) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const query = useInfiniteQuery({
    queryKey: ["core-references"],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{ items: Reference[]; next_cursor: string | null }>(
        `/api/references${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const references = query.data?.pages.flatMap((p) => p.items) ?? [];
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const uploadKey = useRef<{ identity: string; key: string } | undefined>(
    undefined,
  );
  const refresh = async () => {
    await query.refetch();
  };
  return (
    <section className={panelClass}>
      <h2>{ja ? "参照資料の原本" : "Original references"}</h2>
      <p>
        {ja
          ? "原本は非公開で保存されます。抽出に失敗した資料もダウンロードできます。コピーを編集しても原本は変わりません。"
          : "Originals are private and remain downloadable when extraction fails. Editing a working copy preserves the original."}
      </p>
      <Field
        label={
          ja
            ? "PDF・Excel・テキストを追加（10 MiB まで）"
            : "Add PDF, Excel or text (up to 10 MiB)"
        }
      >
        <Input
          type="file"
          accept=".pdf,.xlsx,.txt,.md,.csv"
          disabled={busy}
          onChange={async (e) => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (!file) return;
            const current = ++generation.current;
            setBusy(true);
            setError("");
            try {
              if (file.size === 0 || file.size > 10 * 1024 * 1024)
                throw new Error(
                  ja
                    ? "ファイルは 1 byte～10 MiB にしてください。"
                    : "Files must contain 1 byte to 10 MiB.",
                );
              const bytes = await file.arrayBuffer();
              const digest = await hash(bytes);
              const media_type = file.name.endsWith(".pdf")
                ? "application/pdf"
                : file.name.endsWith(".xlsx")
                  ? "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                  : "text/plain";
              const identity = JSON.stringify([file.name, file.size, digest]);
              if (uploadKey.current?.identity !== identity)
                uploadKey.current = { identity, key: crypto.randomUUID() };
              const reference = await post<Reference>("/references/uploads", {
                idempotency_key: uploadKey.current.key,
                name: file.name,
                media_type,
                size: file.size,
                digest,
              });
              if (reference.state === "uploading") {
                for (
                  let offset =
                    (reference as Reference & { uploaded_bytes: number })
                      .uploaded_bytes ?? 0;
                  offset < bytes.byteLength;
                  offset += 4194304
                ) {
                  let text = "";
                  for (const byte of new Uint8Array(
                    bytes.slice(offset, offset + 4194304),
                  ))
                    text += String.fromCharCode(byte);
                  await post(`/references/${reference.reference_id}/chunks`, {
                    offset,
                    data: btoa(text),
                  });
                }
                await post<Reference>(
                  `/references/${reference.reference_id}/commit`,
                );
              }
              if (current === generation.current) {
                uploadKey.current = undefined;
                await query.refetch();
              }
            } catch (e) {
              setError(String(e));
            } finally {
              setBusy(false);
            }
          }}
        />
      </Field>
      <Hint>
        {ja
          ? "資料に埋め込まれた秘密情報は自動除去されません。抽出結果と利用する Agent を確認してください。"
          : "Embedded secrets are not automatically removed. Check the extraction result and the Agent receiving the reference."}
      </Hint>
      {busy && <Loading>{ja ? "アップロード中…" : "Uploading…"}</Loading>}
      {error && <Alert>{error}</Alert>}
      {query.isError && <Alert>{query.error.message}</Alert>}
      {query.hasNextPage && (
        <Button
          variant="outline"
          type="button"
          onClick={() => void query.fetchNextPage()}
        >
          {ja ? "さらに表示" : "Load more"}
        </Button>
      )}
      {references.map((r) => (
        <article key={r.reference_id}>
          <h3>{r.name}</h3>
          <Hint role="status">
            {r.state} · {r.extraction_state}
          </Hint>
          <Facts
            items={[
              ["ID", r.reference_id, true],
              ["SHA-256", r.digest, true],
            ]}
          />
          <div className={inlineFormClass}>
            {onAttach &&
              (attached.some((a) => a.reference_id === r.reference_id) ? (
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => onDetach?.(r.reference_id)}
                >
                  {ja ? "この Agent から取り外す" : "Detach from this Agent"}
                </Button>
              ) : (
                <Button
                  variant="outline"
                  type="button"
                  disabled={r.state !== "ready" || attached.length >= 8}
                  onClick={() =>
                    onAttach({ reference_id: r.reference_id, digest: r.digest })
                  }
                >
                  {ja ? "この Agent に追加" : "Attach to this Agent"}
                </Button>
              ))}
            <Button
              variant="outline"
              type="button"
              onClick={() => void refresh().catch((e) => setError(String(e)))}
            >
              {ja ? "抽出状態を更新" : "Refresh extraction"}
            </Button>
            <Button
              variant="outline"
              type="button"
              onClick={() =>
                void saveFile(`/references/${r.reference_id}/download`).catch(
                  (e) => setError(String(e)),
                )
              }
            >
              {ja ? "原本をダウンロード" : "Download original"}
            </Button>
            <Button
              variant="outline"
              type="button"
              onClick={async () => {
                try {
                  await post(`/references/${r.reference_id}/revoke`, {
                    expected_revision: r.revision,
                  });
                  await query.refetch();
                } catch (e) {
                  setError(String(e));
                }
              }}
            >
              {ja ? "利用を取り消して原本を削除" : "Revoke and delete original"}
            </Button>
          </div>
        </article>
      ))}
    </section>
  );
}
