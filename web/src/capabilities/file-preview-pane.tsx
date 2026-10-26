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
import { useFileCitations } from "../collaboration/file-citations";
import {
  MAX_FILE_CITATIONS,
  fileCitationFrom,
  fileCitationKey,
} from "../collaboration/file-citation-model";
import { desktop, desktopAvailable } from "../desktop";
import {
  FILE_PREVIEW_BYTE_LIMIT,
  FilePreviewRenderer,
  type FilePreviewVersion,
  classifyFilePreview,
  isImageKind,
  isSpreadsheetKind,
} from "../file-preview";
import { corePreviewTicket } from "../generated/aidash";
import type { PreviewAvailability, PreviewTicket } from "../generated/models";
import { useI18n } from "../ui";
import { download, saveFile, type CoreFile } from "./client";
import { createCurrentManifestResolver } from "./file-preview-source";
import { previewTicketErrorKey } from "./preview-errors";

/** One File version, opened from a Working Area file list or a citation. */
export interface FilePreviewSelection {
  area: string;
  file: CoreFile;
  /** The Working Area revision the file list was read at, when known. */
  revision?: number;
  /** The file list snapshot used to resolve Markdown sibling images. */
  files?: readonly CoreFile[];
  /** The message target a "Reply quoting this version" action belongs to. */
  thread?: string | null;
  /** True when a message citation opened this exact, possibly superseded version. */
  cited?: boolean;
}

export interface FilePreviewContextValue {
  open: (selection: FilePreviewSelection) => void;
  manifestFor: (area: string) => readonly CoreFile[] | undefined;
  /** Null until the session's availability has been read. */
  availability: PreviewAvailability | null;
}

const unavailable: PreviewAvailability = {
  available: false,
  preview_domain: null,
};

const FilePreviewContext = createContext<FilePreviewContextValue>({
  open: () => undefined,
  manifestFor: () => undefined,
  availability: unavailable,
});

export function useFilePreview(): FilePreviewContextValue {
  return useContext(FilePreviewContext);
}

export function FilePreviewContextProvider({
  open,
  manifestFor,
  availability,
  children,
}: {
  open: (selection: FilePreviewSelection) => void;
  manifestFor: (area: string) => readonly CoreFile[] | undefined;
  availability: PreviewAvailability | null;
  children: ReactNode;
}) {
  const value = useMemo(
    () => ({ open, manifestFor, availability }),
    [open, manifestFor, availability],
  );
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

type TicketState =
  | { status: "loading" }
  | { status: "ready"; url: string }
  | { status: "error"; key: string };

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
}: {
  selection: FilePreviewSelection;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const { availability, manifestFor } = useFilePreview();
  const fileCitations = useFileCitations();
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
  const siblingFiles = useMemo(
    () => selection.files ?? manifestFor(selection.area),
    [manifestFor, selection.area, selection.files],
  );
  const files = useMemo(
    () => siblingFiles ?? [selection.file],
    [selection.file, siblingFiles],
  );
  const resolver = useMemo(
    () =>
      createCurrentManifestResolver(selection.area, selection.file.path, files),
    [files, selection.area, selection.file.path],
  );
  const citationTarget = selection.thread ?? "channel";
  const citations = fileCitations.citationsFor(citationTarget);
  const citationKey = fileCitationKey(selection.area, selection.file.file_id);
  const alreadyCited = citations.some(
    (citation) => citation.key === citationKey,
  );
  const citationLimit = citations.length >= MAX_FILE_CITATIONS;

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
  const quote = () => {
    fileCitations.add(
      citationTarget,
      fileCitationFrom(selection.area, selection.file, selection.revision),
    );
  };

  let body: ReactNode;
  if (tooLarge || (state.status === "error" && state.code === "too-large")) {
    body = <Notice tone="warning">{t("filePreviewTooLargeBody")}</Notice>;
  } else if (kind === "preview-domain") {
    if (!availability) {
      body = <Loading>{t("filePreviewCheckingAvailability")}</Loading>;
    } else if (!availability.available) {
      body = (
        <FilePreviewRenderer
          bytes={EMPTY_BYTES}
          mediaType={selection.file.media_type}
          fileName={selection.file.path}
        />
      );
    } else {
      body = (
        <PreviewDomainView selection={selection} availability={availability} />
      );
    }
  } else if (kind === "unsupported") {
    body = (
      <FilePreviewRenderer
        bytes={EMPTY_BYTES}
        mediaType={selection.file.media_type}
        fileName={selection.file.path}
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
        {selection.cited && kind === "markdown" && (
          <Notice className="mb-3">
            {siblingFiles?.length
              ? t("filePreviewCitedCurrentSiblings")
              : t("filePreviewCitedPinnedUnavailable")}
          </Notice>
        )}
        {body}
        {downloadError && <Alert className="mt-2">{downloadError}</Alert>}
      </div>
      <footer className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border px-4 py-3">
        <Button
          variant="outline"
          size="sm"
          type="button"
          disabled={alreadyCited || citationLimit}
          title={citationLimit ? t("fileCitationLimit") : undefined}
          onClick={quote}
        >
          {alreadyCited ? t("fileCitationAlready") : t("fileCitationQuote")}
        </Button>
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

function PreviewDomainView({
  selection,
  availability,
}: {
  selection: FilePreviewSelection;
  availability: PreviewAvailability;
}) {
  if (desktopAvailable())
    return (
      <DesktopPreviewDomain
        selection={selection}
        previewDomain={availability.preview_domain}
      />
    );
  return <WebPreviewDomain selection={selection} />;
}

function WebPreviewDomain({ selection }: { selection: FilePreviewSelection }) {
  const { t } = useI18n();
  const [ticket, setTicket] = useState<TicketState>({ status: "loading" });
  const [retry, setRetry] = useState(0);
  const [opening, setOpening] = useState(false);
  const [actionError, setActionError] = useState("");
  const issued = useRef<Promise<PreviewTicket> | null>(null);

  useEffect(() => {
    let cancelled = false;
    issued.current ??= corePreviewTicket({
      area_id: selection.area,
      file_id: selection.file.file_id,
    });
    void issued.current.then(
      (issued) => {
        if (!cancelled) setTicket({ status: "ready", url: issued.url });
      },
      (error: unknown) => {
        if (!cancelled)
          setTicket({ status: "error", key: previewTicketErrorKey(error) });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [retry, selection.area, selection.file.file_id]);

  const retryTicket = () => {
    issued.current = null;
    setTicket({ status: "loading" });
    setRetry((value) => value + 1);
  };
  const openInNewTab = async () => {
    setOpening(true);
    setActionError("");
    try {
      const issued = await corePreviewTicket({
        area_id: selection.area,
        file_id: selection.file.file_id,
      });
      const anchor = document.createElement("a");
      anchor.href = issued.url;
      anchor.target = "_blank";
      anchor.rel = "noopener noreferrer";
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
    } catch (error) {
      setActionError(t(previewTicketErrorKey(error)));
    } finally {
      setOpening(false);
    }
  };

  if (ticket.status === "loading")
    return <Loading>{t("filePreviewTicketLoading")}</Loading>;
  if (ticket.status === "error")
    return (
      <div className="grid min-w-0 gap-2">
        <Alert>{t(ticket.key)}</Alert>
        <Button
          variant="outline"
          size="sm"
          type="button"
          className="w-fit"
          onClick={retryTicket}
        >
          {t("retry")}
        </Button>
      </div>
    );
  return (
    <div className="grid min-w-0 gap-2">
      <p className="text-xs text-muted-foreground">
        {t("filePreviewPreviewDomainTitle")}
      </p>
      <iframe
        src={ticket.url}
        title={selection.file.path}
        referrerPolicy="no-referrer"
        className="h-[60vh] w-full rounded-md border border-border bg-background"
      />
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          type="button"
          disabled={opening}
          onClick={() => void openInNewTab()}
        >
          {t("filePreviewOpenInNewTab")}
        </Button>
      </div>
      {actionError && <Alert>{actionError}</Alert>}
    </div>
  );
}

function DesktopPreviewDomain({
  selection,
  previewDomain,
}: {
  selection: FilePreviewSelection;
  previewDomain: string | null;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const open = async () => {
    if (!previewDomain) {
      setError(t("filePreviewPreviewUnavailable"));
      return;
    }
    setBusy(true);
    setError("");
    try {
      const issued = await corePreviewTicket({
        area_id: selection.area,
        file_id: selection.file.file_id,
      });
      await desktop.openPreviewUrl(issued.url, previewDomain);
    } catch (caught) {
      setError(t(previewTicketErrorKey(caught)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid min-w-0 gap-2">
      <Notice>
        <strong className="block">{t("filePreviewPreviewDomainTitle")}</strong>
        {t("filePreviewPreviewDomainDesktopBody")}
      </Notice>
      <Button
        variant="outline"
        size="sm"
        type="button"
        className="w-fit"
        disabled={busy}
        onClick={() => void open()}
      >
        {t("filePreviewOpenInBrowser")}
      </Button>
      {error && <Alert>{error}</Alert>}
    </div>
  );
}
