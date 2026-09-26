import { useInfiniteQuery } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { Field, useI18n } from "../ui";
import { SkillImport, type SkillPayload } from "../skill-import";
import { apiFetch } from "../transport";
import { post, saveFile } from "./client";
import "./style.css";
export type Flags = {
  files: boolean;
  shell: boolean;
  python: boolean;
  patch: boolean;
  skills: boolean;
  sharing: boolean;
};
export type SkillAttachment = SkillPayload & {
  skill_id: string;
  origin: string;
  digest: string;
};
export type ReferenceBinding = { reference_id: string; digest: string };
export type CoreConfiguration = {
  core_capabilities: Flags;
  skill_attachments: SkillAttachment[];
  skill_roots: string[];
  reference_attachments: ReferenceBinding[];
};
export const emptyCore: CoreConfiguration = {
  core_capabilities: {
    files: false,
    shell: false,
    python: false,
    patch: false,
    skills: false,
    sharing: false,
  },
  skill_attachments: [],
  skill_roots: [],
  reference_attachments: [],
};
function canonical(value: unknown): unknown {
  return Array.isArray(value)
    ? value.map(canonical)
    : value !== null && typeof value === "object"
      ? Object.fromEntries(
          Object.entries(value)
            .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
            .map(([k, v]) => [k, canonical(v)]),
        )
      : value;
}
async function hash(bytes: ArrayBuffer) {
  return Array.from(
    new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
    (n) => n.toString(16).padStart(2, "0"),
  ).join("");
}
export function CapabilityConfiguration({
  value,
  change,
  privateReferences = false,
}: {
  privateReferences?: boolean;
  value: CoreConfiguration;
  change: (v: CoreConfiguration) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [skill, setSkill] = useState<SkillPayload>();
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const labels: Record<keyof Flags, string> = ja
    ? {
        files: "ファイル検索・閲覧",
        shell: "Shell",
        python: "Python",
        patch: "パッチ適用",
        skills: "直接追加する Skills",
        sharing: "ファイル共有",
      }
    : {
        files: "Search and read files",
        shell: "Shell",
        python: "Python",
        patch: "Apply patches",
        skills: "Direct Skills",
        sharing: "Share files",
      };
  return (
    <fieldset className="core-config">
      <legend>{ja ? "実行機能" : "Execution capabilities"}</legend>
      <p className="muted">
        {ja
          ? "初期状態ではすべて無効です。各機能の利用には現在の権限と実行環境の対応も必要です。"
          : "All capabilities start disabled. Current permissions and a supported runtime are also required."}
      </p>
      <div className="core-flags">
        {Object.entries(labels).map(([key, label]) => (
          <label className="check" key={key}>
            <input
              type="checkbox"
              checked={value.core_capabilities[key as keyof Flags]}
              onChange={(e) => {
                const capability = key as keyof Flags;
                const enabled = e.target.checked;
                change({
                  ...value,
                  ...(capability === "skills" && !enabled
                    ? { skill_attachments: [], skill_roots: [] }
                    : {}),
                  ...(capability === "files" && !enabled
                    ? { reference_attachments: [] }
                    : {}),
                  core_capabilities: {
                    ...value.core_capabilities,
                    [capability]: enabled,
                  },
                });
              }}
            />
            {label}
          </label>
        ))}
      </div>
      {value.core_capabilities.skills && (
        <>
          <SkillImport change={setSkill} />
          <button
            type="button"
            disabled={
              pending ||
              !skill?.instructions ||
              value.skill_attachments.length >= 16
            }
            onClick={async () => {
              if (!skill) return;
              setPending(true);
              setError("");
              try {
                const files = [...skill.files].sort((a, b) =>
                  a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
                );
                const digest =
                  "sha256:" +
                  (await hash(
                    new TextEncoder().encode(
                      JSON.stringify(
                        canonical({ instructions: skill.instructions, files }),
                      ),
                    ).buffer,
                  ));
                change({
                  ...value,
                  skill_attachments: [
                    ...value.skill_attachments,
                    {
                      skill_id: crypto.randomUUID(),
                      origin: skill.source || "upload:SKILL.md",
                      digest,
                      instructions: skill.instructions,
                      files,
                    },
                  ],
                });
              } catch (e) {
                setError(String(e));
              } finally {
                setPending(false);
              }
            }}
          >
            {ja ? "この Skill を追加" : "Attach this Skill"}
          </button>
          <ul>
            {value.skill_attachments.map((a) => (
              <li key={a.skill_id}>
                <strong>{a.origin}</strong>
                <small>{a.digest}</small>
                <button
                  type="button"
                  onClick={() =>
                    change({
                      ...value,
                      skill_attachments: value.skill_attachments.filter(
                        (s) => s.skill_id !== a.skill_id,
                      ),
                    })
                  }
                >
                  {ja ? "取り外す" : "Detach"}
                </button>
              </li>
            ))}
          </ul>
          <label className="check">
            <input
              type="checkbox"
              checked={value.skill_roots.includes(".agents/skills")}
              onChange={(e) =>
                change({
                  ...value,
                  skill_roots: e.target.checked ? [".agents/skills"] : [],
                })
              }
            />
            {ja
              ? "作業ファイル内の .agents/skills を読み込む"
              : "Discover .agents/skills in working files"}
          </label>
        </>
      )}
      {error && <p role="alert">{error}</p>}
      {value.core_capabilities.files && privateReferences && (
        <OriginalReferences
          attached={value.reference_attachments}
          onAttach={(reference) =>
            change({
              ...value,
              reference_attachments: [
                ...value.reference_attachments,
                reference,
              ],
            })
          }
          onDetach={(id) =>
            change({
              ...value,
              reference_attachments: value.reference_attachments.filter(
                (r) => r.reference_id !== id,
              ),
            })
          }
        />
      )}
    </fieldset>
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
    <section className="core-panel">
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
        <input
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
      <p className="muted">
        {ja
          ? "資料に埋め込まれた秘密情報は自動除去されません。抽出結果と利用する Agent を確認してください。"
          : "Embedded secrets are not automatically removed. Check the extraction result and the Agent receiving the reference."}
      </p>
      {busy && <p role="status">{ja ? "アップロード中…" : "Uploading…"}</p>}
      {error && <p role="alert">{error}</p>}
      {query.isError && <p role="alert">{query.error.message}</p>}
      {query.hasNextPage && (
        <button type="button" onClick={() => void query.fetchNextPage()}>
          {ja ? "さらに表示" : "Load more"}
        </button>
      )}
      {references.map((r) => (
        <article key={r.reference_id}>
          <h3>{r.name}</h3>
          <p role="status">
            {r.state} · {r.extraction_state}
          </p>
          <dl>
            <dt>ID</dt>
            <dd>{r.reference_id}</dd>
            <dt>SHA-256</dt>
            <dd>{r.digest}</dd>
          </dl>
          <div className="core-inline">
            {onAttach &&
              (attached.some((a) => a.reference_id === r.reference_id) ? (
                <button
                  type="button"
                  onClick={() => onDetach?.(r.reference_id)}
                >
                  {ja ? "この Agent から取り外す" : "Detach from this Agent"}
                </button>
              ) : (
                <button
                  type="button"
                  disabled={r.state !== "ready" || attached.length >= 8}
                  onClick={() =>
                    onAttach({ reference_id: r.reference_id, digest: r.digest })
                  }
                >
                  {ja ? "この Agent に追加" : "Attach to this Agent"}
                </button>
              ))}
            <button
              type="button"
              onClick={() => void refresh().catch((e) => setError(String(e)))}
            >
              {ja ? "抽出状態を更新" : "Refresh extraction"}
            </button>
            <button
              type="button"
              onClick={() =>
                void saveFile(`/references/${r.reference_id}/download`).catch(
                  (e) => setError(String(e)),
                )
              }
            >
              {ja ? "原本をダウンロード" : "Download original"}
            </button>
            <button
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
            </button>
          </div>
        </article>
      ))}
    </section>
  );
}
