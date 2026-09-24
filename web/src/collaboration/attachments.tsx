import { useEffect, useState } from "react";
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
        <p className="error" role="alert">
          {content.error}
        </p>
      ) : content.image ? (
        <img
          className="workspace-file-image"
          src={content.image}
          alt={attachment.filename}
        />
      ) : content.text !== undefined ? (
        <pre className="workspace-file-text">{content.text}</pre>
      ) : (
        <p role="status">{collaborationCopy[locale].processing}</p>
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
      <div className="workspace-file-card">
        <button
          type="button"
          aria-label={`${copy.downloadAttachment}: ${attachment.filename}`}
          disabled={downloading}
          onClick={() => void download()}
        >
          <FileText size={22} />
          <span>
            <strong>{attachment.filename}</strong>
            <small>
              {attachment.media_type} ·{" "}
              {Math.max(1, Math.ceil(attachment.size_bytes / 1024))} KB
            </small>
          </span>
          <Download size={13} />
        </button>
        {previewable && (
          <button
            type="button"
            aria-label={`${locale === "ja-JP" ? "プレビュー" : "Preview"}: ${attachment.filename}`}
            onClick={() => setPreview(true)}
          >
            <Eye size={14} />
          </button>
        )}
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
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
    <section className="collab-artifacts">
      <h3>
        {locale === "ja-JP"
          ? "チャンネルの共有ファイル"
          : "Shared channel files"}
      </h3>
      {query.isError ? (
        <p role="alert" className="error">
          {copy.unavailable}
        </p>
      ) : (
        <>
          <p className="muted">
            {locale === "ja-JP"
              ? "読み込んだチャンネル履歴の添付ファイルです。返信の添付は各スレッドで確認できます。"
              : "Attachments in loaded channel history. Reply attachments are available in their threads."}
          </p>
          <ul className="collab-attachments workspace-file-gallery">
            {files.map((file) => (
              <li key={file.id}>
                <AttachmentCard workspace={workspace} attachment={file} />
              </li>
            ))}
          </ul>
          {!query.isPending && files.length === 0 && (
            <p className="muted">{copy.empty}</p>
          )}
          {query.hasNextPage && (
            <button
              type="button"
              disabled={query.isFetchingNextPage}
              onClick={() => void query.fetchNextPage()}
            >
              {threadCopy[locale].older}
            </button>
          )}
        </>
      )}
    </section>
  );
}
