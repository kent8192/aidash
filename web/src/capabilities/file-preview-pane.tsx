import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Alert, Loading, Notice } from "../components/patterns";
import { Button } from "../components/ui/button";
import {
  FILE_PREVIEW_BYTE_LIMIT,
  FilePreviewRenderer,
  type FilePreviewVersion,
  classifyFilePreview,
  isImageKind,
  isSpreadsheetKind,
} from "../file-preview";
import { useI18n } from "../ui";
import { download, saveFile, type CoreFile } from "./client";
import { createCurrentManifestResolver } from "./file-preview-source";

/** One File version, opened from a Working Area file list. */
export interface FilePreviewSelection {
  area: string;
  file: CoreFile;
  /** The Working Area revision the file list was read at, when known. */
  revision?: number;
  /** The current file list used to resolve Markdown sibling images. */
  files?: readonly CoreFile[];
}

export interface FilePreviewContextValue {
  open: (selection: FilePreviewSelection) => void;
}

const FilePreviewContext = createContext<FilePreviewContextValue>({
  open: () => undefined,
});

export function useFilePreview(): FilePreviewContextValue {
  return useContext(FilePreviewContext);
}

export function FilePreviewContextProvider({
  open,
  children,
}: {
  open: (selection: FilePreviewSelection) => void;
  children: ReactNode;
}) {
  const value = useMemo(() => ({ open }), [open]);
  return (
    <FilePreviewContext.Provider value={value}>
      {children}
    </FilePreviewContext.Provider>
  );
}

type PreviewState =
  | { status: "loading" }
  | { status: "ready"; version: FilePreviewVersion }
  | { status: "error"; code: "fetch" | "changed" | "too-large" };

const EMPTY_BYTES = new ArrayBuffer(0);

/**
 * The dashboard's File preview pane, shown beside the thread. It reads the
 * selected File version through the same chunked download helper as the other
 * file actions, so the digest check and the 128 MiB browser limit still apply.
 * The pane names the exact File version and focuses its heading when opened;
 * Escape and Close return focus to the action that opened it.
 */
export function FilePreviewPane({
  selection,
  onClose,
  onOpenPreviewDomain,
}: {
  selection: FilePreviewSelection;
  onClose: () => void;
  /**
   * W5b supplies this from the preview-domain ticket API. Until then the pane
   * leaves it unset, so HTML and PDF show the explicit preview-domain state
   * without a button.
   */
  onOpenPreviewDomain?: (fileId: string) => void;
}) {
  const { t } = useI18n();
  const heading = useRef<HTMLHeadingElement>(null);
  const [state, setState] = useState<PreviewState>({ status: "loading" });
  const [retry, setRetry] = useState(0);
  const [downloadError, setDownloadError] = useState("");

  const path = `/working-areas/${selection.area}/files/${selection.file.file_id}/download`;
  const kind = classifyFilePreview(
    selection.file.path,
    selection.file.media_type,
  );
  const needsBytes =
    isImageKind(kind) || kind === "markdown" || isSpreadsheetKind(kind);
  const tooLarge = needsBytes && selection.file.size > FILE_PREVIEW_BYTE_LIMIT;
  const resolver = useMemo(
    () =>
      createCurrentManifestResolver(
        selection.area,
        selection.file.path,
        selection.files ?? [selection.file],
      ),
    [selection.area, selection.file, selection.files],
  );

  useEffect(() => {
    heading.current?.focus();
  }, []);

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  useEffect(() => {
    if (!needsBytes || tooLarge) return;
    let cancelled = false;
    void (async () => {
      try {
        const { blob, file } = await download(path);
        if (cancelled) return;
        if (file.digest !== selection.file.digest) {
          setState({ status: "error", code: "changed" });
          return;
        }
        if (file.size > FILE_PREVIEW_BYTE_LIMIT) {
          setState({ status: "error", code: "too-large" });
          return;
        }
        const bytes = await blob.arrayBuffer();
        if (cancelled) return;
        setState({
          status: "ready",
          version: {
            bytes,
            mediaType: file.media_type,
            fileName: file.path,
          },
        });
      } catch {
        if (!cancelled) setState({ status: "error", code: "fetch" });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [needsBytes, path, retry, selection.file.digest, tooLarge]);

  const retryLoad = () => {
    setState({ status: "loading" });
    setRetry((value) => value + 1);
  };
  const downloadFile = async () => {
    setDownloadError("");
    try {
      await saveFile(path);
    } catch {
      setDownloadError(t("filePreviewPaneDownloadError"));
    }
  };
  const openPreviewDomain = onOpenPreviewDomain
    ? () => onOpenPreviewDomain(selection.file.file_id)
    : undefined;
  let body: ReactNode;
  if (tooLarge || (state.status === "error" && state.code === "too-large")) {
    body = <Notice tone="warning">{t("filePreviewTooLargeBody")}</Notice>;
  } else if (!needsBytes) {
    body = (
      <FilePreviewRenderer
        bytes={EMPTY_BYTES}
        mediaType={selection.file.media_type}
        fileName={selection.file.path}
        onOpenPreviewDomain={openPreviewDomain}
      />
    );
  } else if (state.status === "loading") {
    body = <Loading>{t("filePreviewPaneLoading")}</Loading>;
  } else if (state.status === "error") {
    body = (
      <div className="grid min-w-0 gap-2">
        <Alert>
          {t(
            state.code === "changed"
              ? "filePreviewPaneChanged"
              : "filePreviewPaneFetchError",
          )}
        </Alert>
        {state.code === "fetch" && (
          <Button
            variant="outline"
            size="sm"
            type="button"
            className="w-fit"
            onClick={retryLoad}
          >
            {t("retry")}
          </Button>
        )}
      </div>
    );
  } else {
    body = (
      <FilePreviewRenderer
        bytes={state.version.bytes}
        mediaType={state.version.mediaType}
        fileName={selection.file.path}
        resolveRelativePath={resolver}
        onOpenPreviewDomain={openPreviewDomain}
      />
    );
  }

  return (
    <aside
      aria-label={t("filePreviewPaneTitle")}
      className="absolute inset-y-0 right-0 z-50 flex w-[min(420px,100%)] min-h-0 flex-col overflow-hidden border-l border-border bg-surface shadow-xl focus:outline-none md:w-[400px]"
    >
      <header className="shrink-0 border-b border-border px-4 py-3">
        <h2
          ref={heading}
          tabIndex={-1}
          title={selection.file.path}
          className="truncate font-mono text-[13px] font-semibold text-foreground focus:outline-none"
        >
          {selection.file.path}
        </h2>
        <dl className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
          <div className="flex min-w-0 gap-1">
            <dt>{t("filePreviewPaneDigest")}</dt>
            <dd
              className="truncate font-mono tabular"
              title={selection.file.digest}
            >
              {selection.file.digest.slice(0, 12)}
            </dd>
          </div>
          <div className="flex gap-1">
            <dt className="sr-only">{t("filePreviewPaneSize")}</dt>
            <dd>{selection.file.size.toLocaleString()} bytes</dd>
          </div>
          {selection.revision !== undefined && (
            <div className="flex gap-1">
              <dt>{t("filePreviewPaneRevision")}</dt>
              <dd className="font-mono tabular">{selection.revision}</dd>
            </div>
          )}
        </dl>
      </header>
      <div className="min-h-0 flex-1 overflow-auto p-4">
        {body}
        {downloadError && <Alert className="mt-2">{downloadError}</Alert>}
      </div>
      <footer className="flex shrink-0 items-center gap-2 border-t border-border px-4 py-3">
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={() => void downloadFile()}
        >
          {t("filePreviewDownload")}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          type="button"
          aria-label={t("filePreviewPaneClose")}
          onClick={onClose}
        >
          {t("close")}
        </Button>
      </footer>
    </aside>
  );
}
