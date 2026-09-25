import { ThreadCapabilities } from "../capabilities/thread";
import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import {
  AtSign,
  Bold,
  Code2,
  FileText,
  Italic,
  MessageSquare,
  Paperclip,
  Send,
  X,
} from "lucide-react";
import {
  channelAttachmentUpload,
  channelMessageCreate,
  channelMessageHistory,
  channelThreadCreate,
} from "../generated/aidash";
import type { ChannelAttachment, ChannelMessage } from "../generated/models";
import { AttachmentCard } from "./attachments";
import type { State, Discovery } from "../types";
import { useI18n, useAgentLabel } from "../ui";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";
import { senderLabel } from "./model";
import { threadCopy } from "./thread-copy";
import {
  mergeMessagePages,
  submissionFor,
  validAttachments,
  type MessageSubmission,
} from "./conversation-model";
import { Avatar } from "./avatar";
import "./threads.css";

type DraftFile = { key: string; file: File; uploaded?: ChannelAttachment };
type Draft = { text: string; files: DraftFile[] };
type Drafts = Record<string, Draft>;
const emptyDraft: Draft = { text: "", files: [] };

/** Only inline composer syntax is interpreted. React escapes all other content. */
export function MessageText({ text }: { text: string }) {
  return (
    <>
      {text
        .split(/(\*\*[^*\n]+\*\*|_[^_\n]+_|`[^`\n]+`|@[\p{L}\p{N}_-]+)/gu)
        .map((part, i) =>
          part.startsWith("**") && part.endsWith("**") ? (
            <strong key={i}>{part.slice(2, -2)}</strong>
          ) : part.startsWith("_") && part.endsWith("_") ? (
            <em key={i}>{part.slice(1, -1)}</em>
          ) : part.startsWith("`") && part.endsWith("`") ? (
            <code key={i}>{part.slice(1, -1)}</code>
          ) : part.startsWith("@") ? (
            <span className="workspace-mention" key={i}>
              {part}
            </span>
          ) : (
            part
          ),
        )}
    </>
  );
}

type ConversationProps = {
  data: State;
  discovery?: Discovery;
  workspace: string;
  title: string;
  visible: boolean;
  threadList?: boolean;
  requests?: ReactNode;
  thread: string | null;
  selectThread: (id: string | null) => void;
};
export function ChannelConversation({
  threadContainer,
  ...props
}: ConversationProps & { threadContainer: HTMLDivElement | null }) {
  const [drafts, setDrafts] = useState<Drafts>({});
  const [pending] = useState(() => new Map<string, MessageSubmission>());
  const shared = { ...props, drafts, setDrafts, pending };
  return (
    <>
      <ConversationFeed {...shared} thread={null} />
      {props.thread &&
        threadContainer &&
        createPortal(
          <>
            <ConversationFeed
              key={props.thread}
              {...shared}
              requests={undefined}
              threadList={false}
            />
            {props.data.access.kind === "subject" && (
              <ThreadCapabilities
                key={props.thread}
                workspace={props.workspace}
                thread={props.thread}
                data={props.data}
                onDeleted={() => props.selectThread(null)}
              />
            )}
          </>,
          threadContainer,
        )}
    </>
  );
}

function ConversationFeed({
  workspace,
  title,
  visible,
  data,
  discovery,
  threadList = false,
  requests,
  thread,
  selectThread,
  drafts,
  setDrafts,
  pending,
}: ConversationProps & {
  drafts: Drafts;
  setDrafts: React.Dispatch<React.SetStateAction<Drafts>>;
  pending: Map<string, MessageSubmission>;
}) {
  const { locale, t } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale],
    words = workspaceCopy[locale],
    threads = threadCopy[locale];
  const client = useQueryClient();
  const target = thread ?? "channel",
    draft = drafts[target] ?? emptyDraft;
  const updateDraft = (update: (draft: Draft) => Draft) =>
    setDrafts((current) => ({
      ...current,
      [target]: update(current[target] ?? emptyDraft),
    }));
  const inFlight = useRef(false);
  const [sending, setSending] = useState(false),
    [opening, setOpening] = useState(false),
    [uploading, setUploading] = useState(false);
  const [error, setError] = useState(""),
    [sent, setSent] = useState(false);
  const scroll = useRef<HTMLDivElement>(null),
    textarea = useRef<HTMLTextAreaElement>(null),
    fileInput = useRef<HTMLInputElement>(null);
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
  const allMessages = query.isError
    ? []
    : mergeMessagePages(query.data?.pages ?? []);
  const messages =
    threadList && !thread
      ? allMessages.filter((message) => message.thread_id !== null)
      : allMessages;
  const latestMessageId = messages.at(-1)?.message.id;
  function scrollToLatest() {
    const element = scroll.current;
    if (element) element.scrollTop = element.scrollHeight;
  }
  useEffect(() => {
    if (visible && follow.current && scroll.current)
      scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [latestMessageId, thread, visible]);

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
    if (
      !draft.text.trim() ||
      inFlight.current ||
      query.isError ||
      query.isPending
    )
      return;
    inFlight.current = true;
    setSending(true);
    setError("");
    setSent(false);
    try {
      const attachmentIds: string[] = [];
      for (const entry of draft.files) {
        let uploaded = entry.uploaded;
        if (!uploaded) {
          setUploading(true);
          uploaded = await channelAttachmentUpload(workspace, entry.file, {
            filename: entry.file.name,
            media_type: entry.file.type || "application/octet-stream",
            idempotency_key: entry.key,
          });
          updateDraft((current) => ({
            ...current,
            files: current.files.map((file) =>
              file.key === entry.key ? { ...file, uploaded } : file,
            ),
          }));
        }
        attachmentIds.push(uploaded.id);
      }
      setUploading(false);
      const submission = submissionFor(
        pending.get(target) ?? null,
        workspace,
        thread,
        draft.text,
        () => crypto.randomUUID(),
        attachmentIds,
      );
      pending.set(target, submission);
      await channelMessageCreate(workspace, {
        content: submission.content,
        thread_id: submission.thread,
        idempotency_key: submission.key,
        attachment_ids: submission.attachments,
      });
      updateDraft(() => ({ text: "", files: [] }));
      pending.delete(target);
      setSent(true);
      follow.current = true;
      await Promise.all([
        client.invalidateQueries({ queryKey: ["channel-history", workspace] }),
        client.invalidateQueries({ queryKey: ["workspace", workspace] }),
      ]);
    } catch (reason) {
      setError(
        `${copy.failed} ${reason instanceof Error ? reason.message : String(reason)}`,
      );
    } finally {
      inFlight.current = false;
      setSending(false);
      setUploading(false);
    }
  }
  async function loadOlder() {
    const element = scroll.current;
    if (!element || query.isFetchingNextPage) return;
    follow.current = false;
    setNearBottom(false);
    const previousHeight = element.scrollHeight,
      previousTop = element.scrollTop;
    await query.fetchNextPage();
    requestAnimationFrame(() => {
      if (scroll.current === element)
        element.scrollTop = previousTop + element.scrollHeight - previousHeight;
    });
  }
  function addFiles(files: FileList | null) {
    if (!files) return;
    const next = [
      ...draft.files,
      ...Array.from(files, (file) => ({ file, key: crypto.randomUUID() })),
    ];
    if (!validAttachments(next.map((entry) => entry.file))) {
      setError(words.invalidAttachment);
      return;
    }
    updateDraft((current) => ({ ...current, files: next }));
    setError("");
    setSent(false);
  }
  function insert(prefix: string, suffix = "") {
    const input = textarea.current;
    if (!input) return;
    const start = input.selectionStart,
      end = input.selectionEnd;
    updateDraft((current) => ({
      ...current,
      text:
        current.text.slice(0, start) +
        prefix +
        current.text.slice(start, end) +
        suffix +
        current.text.slice(end),
    }));
    requestAnimationFrame(() => {
      input.focus();
      input.setSelectionRange(start + prefix.length, end + prefix.length);
    });
  }
  const busy = sending || opening || query.isPending;
  return (
    <div
      hidden={!visible}
      className={`collab-conversation-view ${thread ? "is-thread" : ""}`}
    >
      {thread ? (
        <div className="collab-thread-heading">
          <div>
            <h3>{threads.thread}</h3>
            <small># {title}</small>
          </div>
          <button
            type="button"
            disabled={sending || opening}
            aria-label={threads.back}
            onClick={() => selectThread(null)}
          >
            <X size={18} />
          </button>
        </div>
      ) : (
        threadList && (
          <h3 className="workspace-thread-list-title">{words.threads}</h3>
        )
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
              <p className="collab-empty">
                {threadList ? words.noThreads : copy.noMessages}
              </p>
            )}
            {messages.map((entry, index) => {
              const message = entry.message,
                sender = senderLabel(message.sender);
              if (sender.kind === "agent") {
                const separator = sender.name.lastIndexOf("@");
                sender.name =
                  separator < 0
                    ? t("unavailableEntity")
                    : agentLabel(
                        message.sender.slice(
                          0,
                          message.sender.indexOf("/agents/"),
                        ),
                        {
                          id: sender.name.slice(0, separator),
                          version: sender.name.slice(separator + 1),
                        },
                      );
              }
              const date = new Date(message.created_at).toLocaleDateString(
                locale,
                { month: "long", day: "numeric" },
              );
              const previousDate =
                index > 0
                  ? new Date(
                      messages[index - 1].message.created_at,
                    ).toLocaleDateString(locale, {
                      month: "long",
                      day: "numeric",
                    })
                  : "";
              return (
                <Fragment key={message.id}>
                  {date !== previousDate && (
                    <div className="workspace-date-divider">
                      <span>{date}</span>
                    </div>
                  )}
                  <article
                    className={`collab-message ${sender.kind}`}
                    id={`${thread ? "thread-" : ""}message-${message.id}`}
                  >
                    <Avatar
                      name={sender.name}
                      human={sender.kind === "human"}
                    />
                    <div className="workspace-message-body">
                      <div className="collab-sender">
                        <strong>{sender.name}</strong>
                        <span>
                          {sender.kind === "agent" ? "AGENT" : copy.human}
                        </span>
                        {sender.kind === "agent" && (
                          <small>
                            {message.sender.startsWith(`${data.node.id}/`)
                              ? words.local
                              : words.peer}
                          </small>
                        )}
                        <time
                          dateTime={message.created_at}
                          title={new Date(message.created_at).toLocaleString(
                            locale,
                          )}
                        >
                          {new Date(message.created_at).toLocaleTimeString(
                            locale,
                            { hour: "2-digit", minute: "2-digit" },
                          )}
                        </time>
                      </div>
                      <p>
                        <MessageText text={message.content} />
                      </p>
                      {entry.attachments.length > 0 && (
                        <ul className="collab-attachments">
                          {entry.attachments.map((attachment) => (
                            <li key={attachment.id}>
                              <AttachmentCard
                                workspace={workspace}
                                attachment={attachment}
                              />
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
                          <MessageSquare size={12} />
                          {entry.thread_id ? threads.open : threads.reply}
                        </button>
                      )}
                    </div>
                  </article>
                </Fragment>
              );
            })}
            {!thread && !threadList && requests}
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
          {(!threadList || thread) && (
            <form
              className="collab-composer"
              onSubmit={(event) => {
                event.preventDefault();
                void send();
              }}
            >
              <label className="sr-only" htmlFor={`channel-message-${target}`}>
                {thread ? threads.replyMessage : copy.message}
              </label>
              <textarea
                ref={textarea}
                id={`channel-message-${target}`}
                value={draft.text}
                maxLength={64000}
                rows={2}
                placeholder={
                  thread
                    ? threads.replyMessage
                    : `# ${title} — ${copy.placeholder}`
                }
                disabled={busy}
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  updateDraft((current) => ({ ...current, text: value }));
                  setSent(false);
                }}
                onKeyDown={(event) => {
                  if (
                    event.key === "Enter" &&
                    !event.shiftKey &&
                    !event.nativeEvent.isComposing &&
                    event.keyCode !== 229
                  ) {
                    event.preventDefault();
                    void send();
                  }
                }}
              />
              {draft.files.length > 0 && (
                <ul className="workspace-draft-files">
                  {draft.files.map((entry) => (
                    <li key={entry.key}>
                      <FileText size={14} />
                      <span>{entry.file.name}</span>
                      <button
                        type="button"
                        disabled={busy}
                        aria-label={`${words.removeAttachment}: ${entry.file.name}`}
                        onClick={() =>
                          updateDraft((current) => ({
                            ...current,
                            files: current.files.filter(
                              (file) => file.key !== entry.key,
                            ),
                          }))
                        }
                      >
                        <X size={12} />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <div className="collab-composer-bottom">
                <div className="workspace-formatting">
                  <input
                    ref={fileInput}
                    type="file"
                    multiple
                    className="sr-only"
                    aria-label={words.attach}
                    disabled={busy}
                    onChange={(event) => {
                      addFiles(event.target.files);
                      event.target.value = "";
                    }}
                  />
                  <button
                    type="button"
                    disabled={busy}
                    aria-label={words.attach}
                    title={words.attachmentLimit}
                    onClick={() => fileInput.current?.click()}
                  >
                    <Paperclip size={16} />
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    aria-label={words.bold}
                    onClick={() => insert("**", "**")}
                  >
                    <Bold size={14} />
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    aria-label={words.italic}
                    onClick={() => insert("_", "_")}
                  >
                    <Italic size={14} />
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    aria-label={words.code}
                    onClick={() => insert("`", "`")}
                  >
                    <Code2 size={15} />
                  </button>
                  <details className="workspace-mention-picker">
                    <summary aria-label={words.mention}>
                      <AtSign size={15} />
                    </summary>
                    <div>
                      <small>{words.mentionHelp}</small>
                      {data.registry
                        .filter((entry) => entry.kind === "agent")
                        .map((entry) => (
                          <button
                            type="button"
                            disabled={busy}
                            key={`${entry.id}@${entry.version}`}
                            onClick={(event) => {
                              insert(`@${entry.id} `);
                              event.currentTarget
                                .closest("details")
                                ?.removeAttribute("open");
                            }}
                          >
                            {agentLabel(data.node.id, entry)}
                          </button>
                        ))}
                    </div>
                  </details>
                </div>
                <button
                  className="primary"
                  aria-label={thread ? threads.sendReply : copy.send}
                  disabled={busy || !draft.text.trim()}
                >
                  <Send size={15} />
                  {sending && (
                    <span>{uploading ? words.uploading : copy.sending}</span>
                  )}
                </button>
              </div>
            </form>
          )}
          <div className="workspace-composer-hint">
            <span>{words.scope}</span>
            <span>{words.enterHint}</span>
          </div>
        </>
      )}
      {error && (
        <p className="error workspace-message-error" role="alert">
          {error}
        </p>
      )}
      {sent && (
        <span className="sr-only" role="status">
          {copy.sent}
        </span>
      )}
    </div>
  );
}
