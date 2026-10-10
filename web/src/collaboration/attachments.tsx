import { Button } from "../components/ui/button";
import { useEffect, useState } from "react";
import { Alert, Loading } from "../components/patterns";
import { useInfiniteQuery } from "@tanstack/react-query";
import { Download, Eye, FileText } from "lucide-react";
import {
  channelMessageHistory,
  getChannelAttachmentDownloadUrl,
} from "../generated/aidash";
import type { ChannelAttachment } from "../generated/models";
import { authenticatedFetch } from "../transport";
import { Modal, useI18n } from "../ui";
import { collaborationCopy } from "./copy";
import { mergeMessagePages } from "./conversation-model";
import { threadCopy } from "./thread-copy";

function AttachmentPreview({
  workspace,
  attachment,
  close,
}: {
  workspace: string;
  attachment: ChannelAttachment;
  close: () => void;
}) {
  const { locale } = useI18n();
  const [content, setContent] = useState<{
    image?: string;
    text?: string;
    error?: string;
  }>({});
  useEffect(() => {
    const controller = new AbortController();
    let objectUrl: string | undefined;
    void (async () => {
      try {
        const response = await authenticatedFetch(
          getChannelAttachmentDownloadUrl(workspace, attachment.id),
          { signal: controller.signal },
        );
        const blob = await response.blob();
        if (controller.signal.aborted) return;
        if (/^image\/(png|jpeg|gif|webp)$/.test(attachment.media_type)) {
          objectUrl = URL.createObjectURL(blob);
          setContent({ image: objectUrl });
        } else {
          const text = await blob.text();
          if (!controller.signal.aborted) setContent({ text });
        }
      } catch (reason) {
        if (!controller.signal.aborted)
          setContent({
            error: reason instanceof Error ? reason.message : String(reason),
          });
      }
    })();
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [workspace, attachment.id, attachment.media_type]);
  return (
    <Modal title={attachment.filename} close={close}>
      {content.error ? (
        <Alert>{content.error}</Alert>
      ) : content.image ? (
        <img
          className="max-h-[70vh] w-full rounded-md border border-border object-contain"
          src={content.image}
          alt={attachment.filename}
        />
      ) : content.text !== undefined ? (
        <pre className="max-h-[70vh] overflow-auto whitespace-pre-wrap rounded-md border border-border bg-background p-3 font-mono text-xs leading-relaxed text-foreground">
          {content.text}
        </pre>
      ) : (
        <Loading>{collaborationCopy[locale].processing}</Loading>
      )}
    </Modal>
  );
}

export function AttachmentCard({
  workspace,
  attachment,
}: {
  workspace: string;
  attachment: ChannelAttachment;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const [downloading, setDownloading] = useState(false),
    [preview, setPreview] = useState(false),
    [error, setError] = useState("");
  const previewable =
    /^image\/(png|jpeg|gif|webp)$/.test(attachment.media_type) ||
    /^text\//.test(attachment.media_type) ||
    attachment.media_type === "application/json";
  async function download() {
    setDownloading(true);
    setError("");
    try {
      const response = await authenticatedFetch(
        getChannelAttachmentDownloadUrl(workspace, attachment.id),
      );
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement("a");
      link.href = url;
      link.download = attachment.filename;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
    } catch (reason) {
      setError(
        `${copy.downloadFailed} ${reason instanceof Error ? reason.message : String(reason)}`,
      );
    } finally {
      setDownloading(false);
    }
  }
  return (
    <>
      <div className="inline-flex max-w-full items-stretch overflow-hidden rounded-md border border-border bg-surface text-xs">
        <button
          type="button"
          className="flex min-w-0 items-center gap-2 px-2 py-1 text-left transition-colors hover:bg-accent disabled:opacity-50"
          aria-label={`${copy.downloadAttachment}: ${attachment.filename}`}
          disabled={downloading}
          onClick={() => void download()}
        >
          <FileText aria-hidden className="size-3.5 shrink-0 text-faint" />
          <span className="truncate font-mono text-foreground">
            {attachment.filename}
          </span>
          <span className="shrink-0 font-mono tabular text-faint">
            {Math.max(1, Math.ceil(attachment.size_bytes / 1024))} KB
          </span>
          <Download aria-hidden className="size-3 shrink-0 text-faint" />
        </button>
        {previewable && (
          <button
            type="button"
            className="grid w-7 place-items-center border-l border-border text-faint transition-colors hover:bg-accent hover:text-foreground"
            aria-label={`${locale === "ja-JP" ? "プレビュー" : "Preview"}: ${attachment.filename}`}
            onClick={() => setPreview(true)}
          >
            <Eye aria-hidden className="size-3.5" />
          </button>
        )}
      </div>
      {error && <Alert className="mt-1">{error}</Alert>}
      {preview && (
        <AttachmentPreview
          workspace={workspace}
          attachment={attachment}
          close={() => setPreview(false)}
        />
      )}
    </>
  );
}

export function ChannelFiles({ workspace }: { workspace: string }) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const title =
    locale === "ja-JP" ? "チャンネルの共有ファイル" : "Shared channel files";
  const query = useInfiniteQuery({
    queryKey: ["channel-history", workspace, null],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      channelMessageHistory(
        workspace,
        { before: pageParam, limit: 40 },
        { signal },
      ),
    getNextPageParam: (page) => page.next_before ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const files = query.isError
    ? []
    : mergeMessagePages(query.data?.pages ?? []).flatMap(
        (entry) => entry.attachments,
      );
  return (
    <section className="grid gap-2" aria-label={title}>
      <h3 className="text-[11px] font-medium text-faint">{title}</h3>
      {query.isError ? (
        <Alert>{copy.unavailable}</Alert>
      ) : (
        <>
          <p className="text-xs text-muted-foreground">
            {locale === "ja-JP"
              ? "読み込んだチャンネル履歴の添付ファイルです。返信の添付は各スレッドで確認できます。"
              : "Attachments in loaded channel history. Reply attachments are available in their threads."}
          </p>
          <ul className="grid gap-1.5">
            {files.map((file) => (
              <li key={file.id} className="min-w-0">
                <AttachmentCard workspace={workspace} attachment={file} />
              </li>
            ))}
          </ul>
          {!query.isPending && files.length === 0 && (
            <p className="text-xs text-faint">{copy.empty}</p>
          )}
          {query.hasNextPage && (
            <Button
              variant="outline"
              size="sm"
              className="justify-self-start"
              type="button"
              disabled={query.isFetchingNextPage}
              onClick={() => void query.fetchNextPage()}
            >
              {threadCopy[locale].older}
            </Button>
          )}
        </>
      )}
    </section>
  );
}
