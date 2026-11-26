import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Alert, Hint, Loading, RowList } from "./components/patterns";
import {
  FILE_PREVIEW_BYTE_LIMIT,
  FilePreviewRenderer,
  type FilePreviewVersion,
} from "./file-preview";
import { resolveRelativeFilePath } from "./file-preview/relative-path";
import { apiFetch, authenticatedFetch } from "./transport";
import { desktopAvailable } from "./desktop";
import { Field, Modal, Panel, useI18n } from "./ui";

type DriveFile = {
  path: string;
  size: number;
  media_type: string;
  added_in_version: number;
};

type DriveView = {
  drive: { closed: boolean; head: number };
  files: DriveFile[];
  versions: { version: number; author: string; created_at: string }[];
  trash: { id: string; path: string; expires_at: string }[];
  drive_bytes: number;
  scratch_capacity_bytes: number | null;
  scratch_bytes: number | null;
  can_write: boolean;
  can_administer: boolean;
  next_after: string | null;
  index_confirmations: {
    id: string;
    version: number;
    estimated_micros: number;
  }[];
};

type Publication =
  | { status: "published" }
  | { status: "conflict"; paths: string[] };

function driveUrl(node: string): string {
  return `/api/nodes/${encodeURIComponent(node)}/drive`;
}

function fileUrl(node: string, path: string, version: number): string {
  return `${driveUrl(node)}/file?${new URLSearchParams({ path, version: String(version) })}`;
}

function json(method: string, body: unknown): RequestInit {
  return {
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  };
}

export async function moveNodeDrive(source: string, destination: string) {
  await apiFetch(
    driveUrl(source),
    json("POST", { action: "move", destination, confirm: true }),
  );
}

export function NodeDrive({ node }: { node: string }) {
  const { t, locale } = useI18n();
  const [folder, setFolder] = useState("");
  const [version, setVersion] = useState<number>();
  const [after, setAfter] = useState<string>();
  const [uploading, setUploading] = useState(false);
  const [pending, setPending] = useState("");
  const [error, setError] = useState("");
  const [preview, setPreview] = useState<FilePreviewVersion>();
  const url = driveUrl(node);
  const drive = useQuery({
    queryKey: ["node-drive", node, folder, version, after],
    queryFn: () => {
      const query = new URLSearchParams();
      if (folder) query.set("prefix", folder);
      if (version !== undefined) query.set("version", String(version));
      if (after) query.set("after", after);
      return apiFetch<DriveView>(`${url}?${query}`);
    },
    retry: false,
  });
  const data = drive.data;
  const actualVersion = version ?? data?.drive.head;
  const number = new Intl.NumberFormat(locale);
  const record = (value: Publication | { updated: boolean }) => {
    if ("status" in value && value.status === "conflict")
      throw new Error(`${t("driveConflict")} ${value.paths.join(", ")}`);
  };
  async function act(job: () => Promise<void>) {
    setError("");
    setPending(t("driveBusy"));
    try {
      await job();
      await drive.refetch();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setPending("");
    }
  }
  async function mutate(body: unknown) {
    record(
      await apiFetch<Publication | { updated: boolean }>(
        url,
        json("POST", body),
      ),
    );
  }
  async function upload(file: File, destination: string, index: boolean) {
    if (!data) return;
    const path = destination.replace(/^\/+/, "");
    const base = data.files.some((file) => file.path === path)
      ? data.drive.head
      : "new";
    const receipt = await apiFetch<{ id: string; part_bytes: number }>(
      `${url}/uploads`,
      json("POST", { path, size: file.size, base_version: base, index }),
    );
    for (let offset = 0; offset < file.size; offset += receipt.part_bytes) {
      const part = offset / receipt.part_bytes;
      await authenticatedFetch(
        `${url}/uploads/${receipt.id}/chunks?number=${part}`,
        {
          method: "PUT",
          headers: { "content-type": "application/octet-stream" },
          body: file.slice(offset, offset + receipt.part_bytes),
        },
      );
      setPending(
        `${number.format(Math.min(file.size, offset + receipt.part_bytes))} / ${number.format(file.size)} B`,
      );
    }
    record(
      await apiFetch<Publication>(`${url}/uploads/${receipt.id}/finish`, {
        method: "POST",
      }),
    );
    setUploading(false);
  }
  async function download(file: DriveFile) {
    if (actualVersion === undefined) return;
    const href = fileUrl(node, file.path, actualVersion);
    // Browser navigation streams directly to the download target. OPFS isn't
    // required, so authenticated desktop downloads can use the fetch stream.
    if (!desktopAvailable() && !sessionStorage.getItem("aidash-context")) {
      window.location.assign(href);
      return;
    }
    const response = await authenticatedFetch(href);
    const name = file.path.split("/").at(-1) || "file";
    if ("showSaveFilePicker" in window && response.body) {
      const picker = window.showSaveFilePicker as (options: {
        suggestedName: string;
      }) => Promise<{ createWritable(): Promise<WritableStream<Uint8Array>> }>;
      const handle = await picker({ suggestedName: name });
      await response.body.pipeTo(await handle.createWritable());
      return;
    }
    const target = URL.createObjectURL(await response.blob());
    const link = document.createElement("a");
    link.href = target;
    link.download = name;
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(target), 2000);
  }
  async function previewFile(file: DriveFile) {
    if (actualVersion === undefined || file.size > FILE_PREVIEW_BYTE_LIMIT)
      return;
    setPreview({
      bytes: await (
        await authenticatedFetch(fileUrl(node, file.path, actualVersion))
      ).arrayBuffer(),
      fileName: file.path,
      mediaType: file.media_type,
    });
  }
  function openFolder(next: string) {
    setFolder(next);
    setAfter(undefined);
  }
  const rows = new Map<string, DriveFile | null>();
  for (const file of data?.files ?? []) {
    const relative = folder ? file.path.slice(folder.length + 1) : file.path;
    const separator = relative.indexOf("/");
    rows.set(
      separator === -1 ? relative : relative.slice(0, separator),
      separator === -1 ? file : null,
    );
  }
  return (
    <Panel title={t("driveTitle")}>
      {drive.isPending && <Loading />}
      {drive.isError && <Alert>{drive.error.message}</Alert>}
      {error && <Alert>{error}</Alert>}
      {data && (
        <div className="grid gap-4">
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              disabled={!folder}
              onClick={() =>
                openFolder(folder.split("/").slice(0, -1).join("/"))
              }
            >
              {t("driveParentFolder")}
            </Button>
            <span className="font-mono">/{folder}</span>
            <label className="ml-auto flex gap-2">
              {t("driveVersion")}
              <select
                value={version ?? ""}
                onChange={(event) => {
                  setVersion(
                    event.target.value ? Number(event.target.value) : undefined,
                  );
                  setAfter(undefined);
                }}
              >
                <option value="">{t("driveCurrent")}</option>
                {data.versions.map((item) => (
                  <option key={item.version} value={item.version}>
                    {item.version} · {item.author} ·{" "}
                    {new Date(item.created_at).toLocaleDateString(locale)}
                  </option>
                ))}
              </select>
            </label>
            {data.can_write && version === undefined && (
              <Button disabled={!!pending} onClick={() => setUploading(true)}>
                {t("driveUpload")}
              </Button>
            )}
            {data.can_administer && (
              <Button
                variant="outline"
                disabled={!!pending}
                onClick={() =>
                  void act(() =>
                    mutate({ action: "close", closed: !data.drive.closed }),
                  )
                }
              >
                {t(data.drive.closed ? "driveReopen" : "driveClose")}
              </Button>
            )}
          </div>
          <Hint>
            {t("driveStored")} {number.format(data.drive_bytes)} B
            {data.scratch_bytes != null
              ? ` · ${t("driveScratchStored")} ${number.format(data.scratch_bytes)} B / ${number.format(data.scratch_capacity_bytes ?? 0)} B`
              : data.scratch_capacity_bytes != null &&
                ` · ${t("driveScratchCapacity")} ${number.format(data.scratch_capacity_bytes)} B`}
          </Hint>
          {pending && <output>{pending}</output>}
          <RowList>
            {[...rows].map(([name, file]) => (
              <div
                key={name}
                className="flex flex-wrap items-center gap-2 py-2"
              >
                {file ? (
                  <>
                    <span className="min-w-0 flex-1 break-all">{name}</span>
                    <span className="text-xs text-muted-foreground">
                      {number.format(file.size)} B
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={
                        !!pending || file.size > FILE_PREVIEW_BYTE_LIMIT
                      }
                      onClick={() => void act(() => previewFile(file))}
                    >
                      {t("drivePreview")}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={!!pending}
                      onClick={() => void act(() => download(file))}
                    >
                      {t("driveDownload")}
                    </Button>
                    {data.can_write && version === undefined && (
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={!!pending}
                        onClick={() =>
                          void act(() =>
                            mutate({
                              action: "trash",
                              path: file.path,
                              base_version: data.drive.head,
                            }),
                          )
                        }
                      >
                        {t("driveTrash")}
                      </Button>
                    )}
                  </>
                ) : (
                  <Button
                    variant="ghost"
                    onClick={() =>
                      openFolder(folder ? `${folder}/${name}` : name)
                    }
                  >
                    {name}/
                  </Button>
                )}
              </div>
            ))}
          </RowList>
          {data.next_after && (
            <Button
              variant="outline"
              onClick={() => setAfter(data.next_after ?? undefined)}
            >
              {t("driveMore")}
            </Button>
          )}
          {data.trash.length > 0 && (
            <details>
              <summary>{t("driveTrash")}</summary>
              <RowList>
                {data.trash.map((entry) => (
                  <div key={entry.id} className="flex items-center gap-3 py-2">
                    <span className="min-w-0 flex-1 break-all">
                      {entry.path}
                    </span>
                    <time dateTime={entry.expires_at}>
                      {new Date(entry.expires_at).toLocaleDateString(locale)}
                    </time>
                    {data.can_write && (
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={!!pending}
                        onClick={() =>
                          void act(() =>
                            mutate({
                              action: "restore",
                              id: entry.id,
                              base_version: "new",
                            }),
                          )
                        }
                      >
                        {t("driveRestore")}
                      </Button>
                    )}
                  </div>
                ))}
              </RowList>
            </details>
          )}
          {data.index_confirmations.map((request) => (
            <div key={request.id} className="flex items-center gap-2">
              <span>
                {t("driveIndexEstimate")} {request.version} · $
                {new Intl.NumberFormat(locale, {
                  style: "currency",
                  currency: "USD",
                }).format(request.estimated_micros / 1_000_000)}
              </span>
              <Button
                disabled={!!pending}
                onClick={() =>
                  void act(() =>
                    mutate({ action: "confirm_index", id: request.id }),
                  )
                }
              >
                {t("driveIndexConfirm")}
              </Button>
            </div>
          ))}
        </div>
      )}
      {uploading && (
        <Modal
          title={t("driveUpload")}
          close={() => {
            if (!pending) setUploading(false);
          }}
        >
          <form
            className="grid gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              const file = form.get("file");
              const path = String(form.get("path") || "");
              if (file instanceof File)
                void act(() =>
                  upload(
                    file,
                    path || [folder, file.name].filter(Boolean).join("/"),
                    form.has("index"),
                  ),
                );
            }}
          >
            <Field label={t("driveFile")}>
              <Input type="file" name="file" required />
            </Field>
            <Field label={t("driveDestination")}>
              <Input name="path" placeholder={folder ? `${folder}/` : "/"} />
            </Field>
            <label className="flex gap-2">
              <input type="checkbox" name="index" defaultChecked />
              {t("driveIndex")}
            </label>
            {error && <Alert>{error}</Alert>}
            {pending && <output>{pending}</output>}
            <Button disabled={!!pending}>{t("driveUpload")}</Button>
          </form>
        </Modal>
      )}
      {preview && actualVersion !== undefined && (
        <Modal title={preview.fileName} close={() => setPreview(undefined)}>
          <FilePreviewRenderer
            {...preview}
            resolveRelativePath={async (reference) => {
              const path = resolveRelativeFilePath(preview.fileName, reference);
              const sibling = data?.files.find((file) => file.path === path);
              if (
                !sibling ||
                sibling.size > FILE_PREVIEW_BYTE_LIMIT ||
                !sibling.media_type.startsWith("image/")
              )
                return null;
              const response = await authenticatedFetch(
                fileUrl(node, sibling.path, actualVersion),
              );
              return {
                bytes: await response.arrayBuffer(),
                fileName: sibling.path,
                mediaType: sibling.media_type,
              };
            }}
          />
        </Modal>
      )}
    </Panel>
  );
}
