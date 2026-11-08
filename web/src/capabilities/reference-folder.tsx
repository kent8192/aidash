import { useState } from "react";
import { Alert, Hint, Loading } from "../components/patterns";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import { post } from "./client";
import {
  batches,
  folderFiles,
  MAX_FILE_BYTES,
  MAX_REFERENCE_FILES,
  mergeReferences,
  pagesWithoutTextNotice,
  type FolderFile,
} from "./reference-folder-model";

export type ReferenceEntry = {
  reference_id: string;
  digest: string;
  path: string;
};
type AddedReference = {
  reference_id: string;
  state: string;
  pages_without_text?: number[];
  page_count?: number | null;
};
type BatchResult = {
  files: {
    path: string;
    reference: AddedReference | null;
    duplicate_of: string | null;
    error: string | null;
  }[];
  references: ReferenceEntry[];
};

async function base64(file: File) {
  let text = "";
  for (const byte of new Uint8Array(await file.arrayBuffer()))
    text += String.fromCharCode(byte);
  return btoa(text);
}

/** A PDF is read by the parser after the request returns; wait for its pages. */
async function settled(reference: AddedReference) {
  let current = reference;
  for (
    let attempt = 0;
    current.state === "extracting" && attempt < 90;
    attempt++
  ) {
    await new Promise((resolve) => setTimeout(resolve, 2000));
    current = await apiFetch<AddedReference>(
      `/api/references/${reference.reference_id}`,
    );
  }
  return current;
}

/** Adds a whole folder of documents and hands back entries for a Source. */
export function ReferenceFolder({
  onAdded,
}: {
  onAdded: (references: ReferenceEntry[]) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [files, setFiles] = useState<FolderFile<File>[]>([]);
  const [excluded, setExcluded] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notices, setNotices] = useState<string[]>([]);
  const chosen = files.filter((file) => !excluded.has(file.path));
  const add = async () => {
    setBusy(true);
    setError("");
    setNotices([]);
    let references: ReferenceEntry[] = [];
    const messages: string[] = [];
    try {
      const accepted = chosen.filter((file) => {
        if (file.size <= MAX_FILE_BYTES) return true;
        messages.push(
          ja
            ? `${file.path}: 10 MiB を超えているので追加しませんでした。`
            : `${file.path}: over 10 MiB, so it was not added.`,
        );
        return false;
      });
      for (const batch of batches(accepted)) {
        const result = await post<BatchResult>("/references/batch", {
          files: await Promise.all(
            batch.map(async ({ path, file }) => ({
              path,
              data: await base64(file),
            })),
          ),
        });
        references = mergeReferences(references, result.references);
        for (const file of result.files) {
          if (file.duplicate_of)
            messages.push(
              ja
                ? `${file.path}: ${file.duplicate_of} と同じ内容なので 1 件にまとめました。`
                : `${file.path}: same content as ${file.duplicate_of}, so only that one was added.`,
            );
          else if (file.error) messages.push(`${file.path}: ${file.error}`);
          if (!file.reference) continue;
          const reference = await settled(file.reference);
          const notice = pagesWithoutTextNotice(
            reference.pages_without_text,
            reference.page_count,
            ja,
          );
          if (notice) messages.push(`${file.path}: ${notice}`);
        }
      }
      setFiles([]);
      setExcluded(new Set());
    } catch (e) {
      setError(String(e));
    } finally {
      // What was added before a failure stays usable.
      setNotices(messages);
      if (references.length) onAdded(references);
      setBusy(false);
    }
  };
  return (
    <>
      <Field label={ja ? "フォルダを追加" : "Add folder"}>
        <Input
          type="file"
          aria-label={ja ? "フォルダを追加" : "Add folder"}
          disabled={busy}
          // React types omit this attribute; browsers need it to choose a folder.
          {...{ webkitdirectory: "" }}
          onChange={(e) => {
            setFiles(
              folderFiles(
                Array.from(e.target.files ?? [], (file) => ({
                  relativePath: file.webkitRelativePath,
                  size: file.size,
                  file,
                })),
              ),
            );
            setExcluded(new Set());
            setNotices([]);
            e.target.value = "";
          }}
        />
      </Field>
      {files.length > 0 && (
        <>
          <Hint>
            {ja
              ? "隠しファイルと空のファイルは除きました。秘密情報は自動で検出されません。トークンなどを含むファイルは外してください。"
              : "Hidden and empty files were left out. Secrets are not detected. Remove any file holding tokens or passwords."}
          </Hint>
          <ul>
            {files.map(({ path }) => (
              <li key={path}>
                <label>
                  <input
                    type="checkbox"
                    checked={!excluded.has(path)}
                    onChange={(e) => {
                      const next = new Set(excluded);
                      if (e.target.checked) next.delete(path);
                      else next.add(path);
                      setExcluded(next);
                    }}
                  />{" "}
                  {path}
                </label>
              </li>
            ))}
          </ul>
          {chosen.length > MAX_REFERENCE_FILES && (
            <Alert>
              {ja
                ? `1 つの Agent に渡せるのは ${MAX_REFERENCE_FILES} 件までです。`
                : `One Agent can have at most ${MAX_REFERENCE_FILES} documents.`}
            </Alert>
          )}
          <Button
            type="button"
            disabled={
              busy || chosen.length === 0 || chosen.length > MAX_REFERENCE_FILES
            }
            onClick={() => void add()}
          >
            {ja
              ? `${chosen.length} 件を追加`
              : `Add ${chosen.length} documents`}
          </Button>
        </>
      )}
      {busy && <Loading>{ja ? "追加しています…" : "Adding…"}</Loading>}
      {error && <Alert>{error}</Alert>}
      {notices.map((notice) => (
        <Hint key={notice} role="status">
          {notice}
        </Hint>
      ))}
    </>
  );
}
