import { apiFetch } from "../transport";
export type CoreFile = {
  file_id: string;
  path: string;
  digest: string;
  size: number;
  media_type: string;
  scope: "working" | "references" | "received";
};
export type Area = {
  id: string;
  workspace_id: string;
  thread_id: string;
  agent_id: string;
  state: string;
  revision: number;
  generation: number;
  manifest: CoreFile[];
};
export type Operation = {
  operation_id: string;
  area_id?: string;
  status: string;
  kind?: string;
  output?: string;
  error?: { code: string; message: string; retryable: boolean };
  session_id?: string;
  session_reset?: boolean;
  reset_reason?: string;
  next_offset?: number;
  effects_may_have_occurred?: boolean;
  termination_confirmed?: boolean;
  writer_frozen?: boolean;
  receipt?: { files: CoreFile[] };
  displays?: CoreFile[];
};
export const post = <T>(path: string, input: unknown = {}) =>
  apiFetch<T>(`/api${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
export async function download(
  path: string,
): Promise<{ blob: Blob; file: CoreFile }> {
  const parts: Uint8Array<ArrayBuffer>[] = [];
  let offset: number | null = 0;
  let file: CoreFile | undefined;
  while (offset !== null) {
    const page: { file: CoreFile; data: string; next_offset: number | null } =
      await apiFetch(`/api${path}?offset=${offset}`);
    file ??= page.file;
    if (
      file.file_id !== page.file.file_id ||
      file.digest !== page.file.digest ||
      file.size > 100 * 1024 * 1024
    )
      throw new Error("File changed or exceeds download limit");
    parts.push(Uint8Array.from(atob(page.data), (c) => c.charCodeAt(0)));
    if (page.next_offset !== null && page.next_offset <= offset)
      throw new Error("Invalid download continuation");
    offset = page.next_offset;
  }
  if (!file) throw new Error("File unavailable");
  const blob = new Blob(parts, { type: file.media_type });
  const digest = Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()),
    ),
    (n) => n.toString(16).padStart(2, "0"),
  ).join("");
  if (blob.size !== file.size || digest !== file.digest)
    throw new Error("File integrity check failed");
  return { blob, file };
}
export async function saveFile(path: string) {
  const { blob, file } = await download(path);
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = file.path.split("/").at(-1) ?? "download";
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
