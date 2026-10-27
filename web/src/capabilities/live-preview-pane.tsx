import { useEffect, useRef, useState } from "react";
import { Alert, Loading, Notice } from "../components/patterns";
import { Button } from "../components/ui/button";
import { desktop, desktopAvailable } from "../desktop";
import { corePreviewLiveTicket } from "../generated/aidash";
import type { PreviewTicket } from "../generated/models";
import { useI18n } from "../ui";
import { useFilePreview, type LivePreviewSelection } from "./file-preview-pane";
import { livePreviewTicketErrorKey } from "./live-preview-model";

/**
 * A Live preview in the same side pane as a File preview. Each view and each
 * new-tab action requests a fresh one-time ticket; the URL is never rendered
 * as text and the iframe has no sandbox attribute (the gateway sets
 * frame-ancestors to this dashboard).
 */
export function LivePreviewPane({
  selection,
  onClose,
}: {
  selection: LivePreviewSelection;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const { availability } = useFilePreview();
  const heading = useRef<HTMLHeadingElement>(null);

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

  return (
    <aside
      aria-label={t("livePreviewTitle")}
      className="absolute inset-y-0 right-0 z-50 flex w-[min(420px,100%)] min-h-0 flex-col overflow-hidden border-l border-border bg-surface shadow-xl focus:outline-none md:w-[400px]"
    >
      <header className="shrink-0 border-b border-border px-4 py-3">
        <h2
          ref={heading}
          tabIndex={-1}
          className="truncate text-[13px] font-semibold text-foreground focus:outline-none"
        >
          {selection.title ?? t("livePreviewTitle")}
        </h2>
        <p className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
          {selection.run}
        </p>
      </header>
      <div className="min-h-0 flex-1 overflow-auto p-4">
        {!availability ? (
          <Loading>{t("filePreviewCheckingAvailability")}</Loading>
        ) : !availability.available ? (
          <Notice tone="warning">{t("livePreviewUnavailable")}</Notice>
        ) : desktopAvailable() ? (
          <DesktopLivePreview selection={selection} />
        ) : (
          <WebLivePreview selection={selection} />
        )}
      </div>
      <footer className="flex shrink-0 items-center gap-2 border-t border-border px-4 py-3">
        <Button
          variant="ghost"
          size="sm"
          type="button"
          aria-label={t("livePreviewClose")}
          onClick={onClose}
        >
          {t("close")}
        </Button>
      </footer>
    </aside>
  );
}

function WebLivePreview({ selection }: { selection: LivePreviewSelection }) {
  const { t } = useI18n();
  const [ticket, setTicket] = useState<
    | { status: "loading" }
    | { status: "ready"; url: string }
    | { status: "error"; key: string }
  >({ status: "loading" });
  const [retry, setRetry] = useState(0);
  const [opening, setOpening] = useState(false);
  const [actionError, setActionError] = useState("");
  const issued = useRef<Promise<PreviewTicket> | null>(null);

  useEffect(() => {
    let cancelled = false;
    issued.current ??= corePreviewLiveTicket({
      live_preview_id: selection.livePreviewId,
    });
    void issued.current.then(
      (value) => {
        if (!cancelled) setTicket({ status: "ready", url: value.url });
      },
      (error: unknown) => {
        if (!cancelled)
          setTicket({ status: "error", key: livePreviewTicketErrorKey(error) });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [retry, selection.livePreviewId]);

  const retryTicket = () => {
    issued.current = null;
    setTicket({ status: "loading" });
    setRetry((value) => value + 1);
  };
  const openInNewTab = async () => {
    setOpening(true);
    setActionError("");
    try {
      const value = await corePreviewLiveTicket({
        live_preview_id: selection.livePreviewId,
      });
      const anchor = document.createElement("a");
      anchor.href = value.url;
      anchor.target = "_blank";
      anchor.rel = "noopener noreferrer";
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
    } catch (error) {
      setActionError(t(livePreviewTicketErrorKey(error)));
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
      <iframe
        src={ticket.url}
        title={selection.title ?? t("livePreviewTitle")}
        referrerPolicy="no-referrer"
        className="h-[60vh] w-full rounded-md border border-border bg-background"
      />
      <Button
        variant="outline"
        size="sm"
        type="button"
        className="w-fit"
        disabled={opening}
        onClick={() => void openInNewTab()}
      >
        {t("filePreviewOpenInNewTab")}
      </Button>
      {actionError && <Alert>{actionError}</Alert>}
    </div>
  );
}

function DesktopLivePreview({
  selection,
}: {
  selection: LivePreviewSelection;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const open = async () => {
    setBusy(true);
    setError("");
    try {
      const value = await corePreviewLiveTicket({
        live_preview_id: selection.livePreviewId,
      });
      await desktop.openPreviewUrl(value.url);
    } catch (caught) {
      setError(t(livePreviewTicketErrorKey(caught)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid min-w-0 gap-2">
      <Notice>{t("livePreviewDesktopBody")}</Notice>
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
