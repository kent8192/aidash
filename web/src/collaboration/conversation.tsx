import { useEffect, useRef, useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import {
  channelMessageCreate,
  channelMessageHistory,
  channelThreadCreate,
  getChannelAttachmentDownloadUrl,
} from "../generated/aidash";
import type { ChannelAttachment, ChannelMessage } from "../generated/models";
import { authenticatedFetch } from "../transport";
import { useI18n } from "../ui";
import { collaborationCopy } from "./copy";
import { senderLabel } from "./model";
import { threadCopy } from "./thread-copy";
import {
  mergeMessagePages,
  submissionFor,
  type MessageSubmission,
} from "./conversation-model";
import "./threads.css";

export function ChannelConversation({
  workspace,
  visible,
}: {
  workspace: string;
  visible: boolean;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const threads = threadCopy[locale];
  const client = useQueryClient();
  const [thread, setThread] = useState<string | null>(null);
  const target = thread ?? "channel";
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const draft = drafts[target] ?? "";
  const pending = useRef(new Map<string, MessageSubmission>());
  const inFlight = useRef(false);
  const [sending, setSending] = useState(false);
  const [opening, setOpening] = useState(false);
  const [downloading, setDownloading] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [sent, setSent] = useState(false);
  const scroll = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const [nearBottom, setNearBottom] = useState(true);
  const query = useInfiniteQuery({
    queryKey: ["channel-history", workspace, thread],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      channelMessageHistory(
        workspace,
        { thread_id: thread ?? undefined, before: pageParam, limit: 40 },
        { signal },
      ),
    getNextPageParam: (page) => page.next_before ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  // Failed authorization refreshes hide every page, including cached history.
  const messages = query.isError
    ? []
    : mergeMessagePages(query.data?.pages ?? []);
  const latestMessageId = messages.at(-1)?.message.id;
  function scrollToLatest() {
    const element = scroll.current;
    if (element) element.scrollTop = element.scrollHeight;
  }
  useEffect(() => {
    if (visible && follow.current && scroll.current)
      scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [latestMessageId, thread, visible]);

  function selectThread(id: string | null) {
    setThread(id);
    setError("");
    setSent(false);
    follow.current = true;
    setNearBottom(true);
  }

  async function openThread(message: ChannelMessage) {
    if (inFlight.current) return;
    if (message.thread_id) {
      selectThread(message.thread_id);
      return;
    }
    inFlight.current = true;
    setOpening(true);
    setError("");
    try {
      const opened = await channelThreadCreate(workspace, {
        root_message_id: message.message.id,
      });
      selectThread(opened.id);
      await client.invalidateQueries({
        queryKey: ["channel-history", workspace],
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setOpening(false);
      inFlight.current = false;
    }
  }

  async function send() {
    if (!draft.trim() || inFlight.current || query.isError || query.isPending)
      return;
    const submission = submissionFor(
      pending.current.get(target) ?? null,
      workspace,
      thread,
      draft,
      () => crypto.randomUUID(),
    );
    pending.current.set(target, submission);
    inFlight.current = true;
    setSending(true);
    setError("");
    setSent(false);
    try {
      await channelMessageCreate(workspace, {
        content: submission.content,
        thread_id: submission.thread,
        idempotency_key: submission.key,
      });
      setDrafts((current) => ({ ...current, [target]: "" }));
      pending.current.delete(target);
      setSent(true);
      follow.current = true;
      await client.invalidateQueries({
        queryKey: ["channel-history", workspace],
      });
      await client.invalidateQueries({ queryKey: ["workspace", workspace] });
    } catch (reason) {
      setError(
        `${copy.failed} ${reason instanceof Error ? reason.message : String(reason)}`,
      );
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  }

  async function downloadAttachment(attachment: ChannelAttachment) {
    setDownloading(attachment.id);
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
      setDownloading(null);
    }
  }

  async function loadOlder() {
    const element = scroll.current;
    if (!element || query.isFetchingNextPage) return;
    follow.current = false;
    setNearBottom(false);
    const previousHeight = element.scrollHeight;
    const previousTop = element.scrollTop;
    await query.fetchNextPage();
    requestAnimationFrame(() => {
      if (scroll.current === element)
        element.scrollTop = previousTop + element.scrollHeight - previousHeight;
    });
  }

  return (
    <div hidden={!visible} className="collab-conversation-view">
      {thread && (
        <div className="collab-thread-heading">
          <button
            type="button"
            disabled={sending || opening}
            onClick={() => selectThread(null)}
          >
            {threads.back}
          </button>
          <h3>{threads.thread}</h3>
        </div>
      )}
      {query.isError ? (
        <div className="error" role="alert">
          <p>{copy.unavailable}</p>
          <p>{query.error.message}</p>
          <button type="button" onClick={() => void query.refetch()}>
            {copy.retry}
          </button>
        </div>
      ) : (
        <>
          <div
            ref={scroll}
            className="collab-messages"
            aria-label={thread ? threads.thread : copy.conversation}
            onScroll={() => {
              const element = scroll.current;
              if (element) {
                follow.current =
                  element.scrollHeight -
                    element.scrollTop -
                    element.clientHeight <
                  80;
                setNearBottom(follow.current);
              }
            }}
          >
            {query.hasNextPage && (
              <button
                type="button"
                disabled={query.isFetchingNextPage}
                onClick={() => void loadOlder()}
              >
                {threads.older}
              </button>
            )}
            {query.isPending && <p role="status">{copy.processing}</p>}
            {!query.isPending && messages.length === 0 && (
              <p className="collab-empty">{copy.noMessages}</p>
            )}
            {messages.map((entry) => {
              const message = entry.message;
              const sender = senderLabel(message.sender);
              return (
                <article
                  className={`collab-message ${sender.kind}`}
                  key={message.id}
                  id={`message-${message.id}`}
                >
                  <div className="collab-sender">
                    <strong>{sender.name}</strong>
                    <span>{copy[sender.kind]}</span>
                    <time dateTime={message.created_at}>
                      {new Date(message.created_at).toLocaleString(locale)}
                    </time>
                  </div>
                  <p>{message.content}</p>
                  {entry.attachments.length > 0 && (
                    <ul className="collab-attachments">
                      {entry.attachments.map((attachment) => (
                        <li key={attachment.id}>
                          <button
                            type="button"
                            aria-label={`${copy.downloadAttachment}: ${attachment.filename}`}
                            disabled={downloading === attachment.id}
                            onClick={() => void downloadAttachment(attachment)}
                          >
                            {attachment.filename}
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                  {!thread && (
                    <button
                      className="collab-thread-action"
                      type="button"
                      disabled={sending || opening}
                      onClick={() => void openThread(entry)}
                    >
                      {entry.thread_id ? threads.open : threads.reply}
                    </button>
                  )}
                </article>
              );
            })}
          </div>
          {!nearBottom && (
            <button
              className="collab-latest"
              type="button"
              onClick={() => {
                follow.current = true;
                setNearBottom(true);
                scrollToLatest();
              }}
            >
              {copy.newMessages}
            </button>
          )}
          {opening && <p role="status">{threads.opening}</p>}
          <form
            className="collab-composer"
            onSubmit={(event) => {
              event.preventDefault();
              void send();
            }}
          >
            <label className="sr-only" htmlFor="channel-message">
              {copy.message}
            </label>
            <textarea
              id="channel-message"
              value={draft}
              maxLength={64000}
              rows={3}
              placeholder={copy.placeholder}
              disabled={sending || opening || query.isPending}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setDrafts((current) => ({ ...current, [target]: value }));
                setSent(false);
              }}
              onKeyDown={(event) => {
                if (
                  (event.ctrlKey || event.metaKey) &&
                  event.key === "Enter" &&
                  !event.nativeEvent.isComposing
                ) {
                  event.preventDefault();
                  void send();
                }
              }}
            />
            <div className="collab-composer-bottom">
              <small>Ctrl / ⌘ + Enter</small>
              <button
                className="primary"
                disabled={
                  sending || opening || query.isPending || !draft.trim()
                }
              >
                {sending ? copy.sending : copy.send}
              </button>
            </div>
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            {sent && (
              <p className="muted" role="status">
                {copy.sent}
              </p>
            )}
          </form>
        </>
      )}
    </div>
  );
}
