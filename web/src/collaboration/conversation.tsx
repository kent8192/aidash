import { Button } from "../components/ui/button";
import { ThreadCapabilities } from "../capabilities/thread";
import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Alert, Loading } from "../components/patterns";
import { createPortal } from "react-dom";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDown,
  AtSign,
  Bold,
  Code2,
  FileText,
  Italic,
  MessageSquare,
  MessagesSquare,
  Paperclip,
  SendHorizontal,
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
import { cn } from "../lib/utils";
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
import { Textarea } from "../components/ui/textarea";
import { Kbd } from "../components/ui/kbd";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/popover";

type DraftFile = { key: string; file: File; uploaded?: ChannelAttachment };
type Draft = { text: string; files: DraftFile[] };
type Drafts = Record<string, Draft>;
const emptyDraft: Draft = { text: "", files: [] };

/** A workspace event shown inline in the channel feed as a muted system line. */
export type SystemLine = { id: string; created_at: string; text: string };

function latestMessageTop(scroll: HTMLElement, message: HTMLElement | null) {
  if (!message) return 0;
  const end =
    message.getBoundingClientRect().bottom -
    scroll.getBoundingClientRect().top +
    scroll.scrollTop -
    scroll.clientTop;
  return Math.max(0, end - scroll.clientHeight);
}

/** Only inline composer syntax is interpreted. React escapes all other content. */
export function MessageText({ text }: { text: string }) {
  return (
    <>
      {text
        .split(/(\*\*[^*\n]+\*\*|_[^_\n]+_|`[^`\n]+`|@[\p{L}\p{N}_-]+)/gu)
        .map((part, i) =>
          part.startsWith("**") && part.endsWith("**") ? (
            <strong key={i} className="font-semibold">
              {part.slice(2, -2)}
            </strong>
          ) : part.startsWith("_") && part.endsWith("_") ? (
            <em key={i}>{part.slice(1, -1)}</em>
          ) : part.startsWith("`") && part.endsWith("`") ? (
            <code
              key={i}
              className="rounded-sm bg-raised px-1 py-px font-mono text-[12px]"
            >
              {part.slice(1, -1)}
            </code>
          ) : part.startsWith("@") ? (
            <span className="font-medium text-brand" key={i}>
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
  /** Rendered after the channel messages (pending decisions on narrow screens). */
  requests?: ReactNode;
  system?: SystemLine[];
  thread: string | null;
  selectThread: (id: string | null) => void;
};
export function ChannelConversation({
  threadContainer,
  ...props
}: ConversationProps & { threadContainer: HTMLElement | null }) {
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
              system={undefined}
              threadList={false}
            />
            {props.data.access.kind === "subject" && (
              <div className="max-h-[45%] shrink-0 overflow-y-auto border-t border-border px-4 py-3">
                <ThreadCapabilities
                  key={props.thread}
                  workspace={props.workspace}
                  thread={props.thread}
                  data={props.data}
                  onDeleted={() => props.selectThread(null)}
                />
              </div>
            )}
          </>,
          threadContainer,
        )}
    </>
  );
}

const toolButton =
  "size-7 text-muted-foreground hover:text-foreground [&_svg]:size-3.5";

function ConversationFeed({
  workspace,
  title,
  visible,
  data,
  discovery,
  threadList = false,
  requests,
  system,
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
    [uploading, setUploading] = useState(false),
    [mentioning, setMentioning] = useState(false);
  const [error, setError] = useState(""),
    [sent, setSent] = useState(false);
  const scroll = useRef<HTMLDivElement>(null),
    latest = useRef<HTMLElement>(null),
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
  const scrollToLatest = useCallback(() => {
    const element = scroll.current;
    if (element) element.scrollTop = latestMessageTop(element, latest.current);
  }, []);
  useEffect(() => {
    if (visible && follow.current) scrollToLatest();
  }, [latestMessageId, thread, visible, scrollToLatest]);

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
      (!draft.text.trim() && draft.files.length === 0) ||
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
  const day = (value: string) =>
    new Date(value).toLocaleDateString(locale, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
  const clock = (value: string) =>
    new Date(value).toLocaleTimeString(locale, {
      hour: "2-digit",
      minute: "2-digit",
    });
  const firstShown = messages[0]?.message.created_at;
  const systemLines =
    !thread && !threadList && firstShown
      ? (system ?? []).filter((line) => line.created_at >= firstShown)
      : [];
  const feed = [
    ...messages.map((entry) => ({
      kind: "message" as const,
      at: entry.message.created_at,
      entry,
    })),
    ...systemLines.map((line) => ({
      kind: "system" as const,
      at: line.created_at,
      line,
    })),
  ].sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  return (
    <div hidden={!visible} className="relative flex min-h-0 flex-1 flex-col">
      {thread ? (
        <div className="flex h-11 shrink-0 items-center gap-2 border-b border-border px-4">
          <MessagesSquare aria-hidden className="size-4 text-faint" />
          <div className="grid min-w-0 flex-1">
            <h3 className="text-[13px] font-semibold leading-tight text-foreground">
              {threads.thread}
            </h3>
            <small className="truncate font-mono text-[11px] text-faint">
              # {title}
            </small>
          </div>
          <Button
            variant="ghost"
            size="icon"
            type="button"
            disabled={sending || opening}
            aria-label={threads.back}
            onClick={() => selectThread(null)}
          >
            <X />
          </Button>
        </div>
      ) : (
        threadList && (
          <h3 className="shrink-0 border-b border-border px-4 py-2.5 text-xs font-semibold text-foreground">
            {words.threads}
          </h3>
        )
      )}
      {query.isError ? (
        <Alert
          className="m-4"
          retry={() => void query.refetch()}
          retryLabel={copy.retry}
        >
          <p className="font-medium">{copy.unavailable}</p>
          <p className="break-words text-muted-foreground">
            {query.error.message}
          </p>
        </Alert>
      ) : (
        <>
          <div
            ref={scroll}
            role="region"
            className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 py-3"
            aria-label={thread ? threads.thread : copy.conversation}
            onScroll={() => {
              const element = scroll.current;
              if (element) {
                follow.current =
                  Math.abs(
                    latestMessageTop(element, latest.current) -
                      element.scrollTop,
                  ) < 80;
                setNearBottom(follow.current);
              }
            }}
          >
            {query.hasNextPage && (
              <Button
                variant="ghost"
                size="sm"
                className="mx-auto mb-2 flex text-xs"
                type="button"
                disabled={query.isFetchingNextPage}
                onClick={() => void loadOlder()}
              >
                {threads.older}
              </Button>
            )}
            {query.isPending && (
              <Loading className="py-6 text-center">{copy.processing}</Loading>
            )}
            {!query.isPending && messages.length === 0 && (
              <div className="mx-auto mt-8 grid max-w-64 justify-items-center gap-2 text-center">
                <span className="grid size-8 place-items-center rounded-md bg-raised text-faint">
                  <MessageSquare aria-hidden className="size-4" />
                </span>
                <p className="text-[13px] font-medium text-foreground">
                  {threadList ? words.noThreads : copy.noMessages}
                </p>
                {!threadList && (
                  <p className="text-xs text-muted-foreground">{words.scope}</p>
                )}
              </div>
            )}
            {feed.map((item, index) => {
              const previous = feed[index - 1];
              const divider =
                !previous || day(previous.at) !== day(item.at) ? (
                  <div className="my-2 flex items-center gap-3 font-mono text-[11px] tabular text-faint before:h-px before:flex-1 before:bg-border after:h-px after:flex-1 after:bg-border">
                    {day(item.at)}
                  </div>
                ) : null;
              if (item.kind === "system")
                return (
                  <Fragment key={`system-${item.line.id}`}>
                    {divider}
                    <p className="my-1.5 ml-3 flex gap-2 border-l border-border py-0.5 pl-3 text-xs text-muted-foreground">
                      <time
                        dateTime={item.at}
                        className="shrink-0 font-mono tabular text-faint"
                      >
                        {clock(item.at)}
                      </time>
                      <span className="min-w-0">{item.line.text}</span>
                    </p>
                  </Fragment>
                );
              const entry = item.entry;
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
              const remote =
                sender.kind === "agent" &&
                !message.sender.startsWith(`${data.node.id}/`);
              return (
                <Fragment key={message.id}>
                  {divider}
                  <article
                    ref={message.id === latestMessageId ? latest : undefined}
                    className="group grid grid-cols-[24px_1fr] gap-x-2.5 rounded-md py-2"
                    id={`${thread ? "thread-" : ""}message-${message.id}`}
                  >
                    <Avatar
                      name={sender.name}
                      human={sender.kind === "human"}
                      className="mt-0.5 size-6 text-[11px]"
                    />
                    <div className="min-w-0">
                      <header className="flex items-baseline gap-2">
                        <span className="truncate text-[13px] font-semibold text-foreground">
                          {sender.name}
                        </span>
                        {remote && (
                          <span className="shrink-0 rounded-sm border border-brand-line px-1 text-[10px] leading-4 text-brand">
                            {words.peer}
                          </span>
                        )}
                        <span className="sr-only">
                          {sender.kind === "agent" ? copy.agent : copy.human}
                        </span>
                        <time
                          className="shrink-0 font-mono text-[11px] tabular text-faint"
                          dateTime={message.created_at}
                          title={new Date(message.created_at).toLocaleString(
                            locale,
                          )}
                        >
                          {clock(message.created_at)}
                        </time>
                      </header>
                      <p className="whitespace-pre-wrap break-words text-[13px] leading-relaxed text-foreground">
                        <MessageText text={message.content} />
                      </p>
                      {entry.attachments.length > 0 && (
                        <ul className="mt-1.5 flex flex-wrap gap-1.5">
                          {entry.attachments.map((attachment) => (
                            <li key={attachment.id} className="min-w-0">
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
                          className={cn(
                            "mt-1.5 inline-flex h-6 items-center gap-1.5 rounded-md border border-border px-2 text-[11px] text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground disabled:opacity-50",
                            !entry.thread_id &&
                              "md:opacity-0 md:group-hover:opacity-100 md:focus-visible:opacity-100",
                          )}
                          type="button"
                          disabled={sending || opening}
                          onClick={() => void openThread(entry)}
                        >
                          <MessagesSquare aria-hidden className="size-3" />
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
            <Button
              variant="outline"
              size="sm"
              className="absolute bottom-[calc(var(--composer-h,9rem)+0.5rem)] left-1/2 z-10 -translate-x-1/2 rounded-full bg-popover shadow-overlay"
              type="button"
              onClick={() => {
                follow.current = true;
                setNearBottom(true);
                scrollToLatest();
              }}
            >
              <ArrowDown />
              {copy.newMessages}
            </Button>
          )}
          {opening && (
            <Loading className="px-4 pt-0 pb-1">{threads.opening}</Loading>
          )}
          {error && <Alert className="mx-4 mb-2">{error}</Alert>}
          {(!threadList || thread) && (
            <form
              className="shrink-0 border-t border-border p-3"
              onSubmit={(event) => {
                event.preventDefault();
                void send();
              }}
            >
              <div className="rounded-md border border-input bg-background transition-colors focus-within:border-brand-line">
                <label
                  className="sr-only"
                  htmlFor={`channel-message-${target}`}
                >
                  {thread ? threads.replyMessage : copy.message}
                </label>
                <Textarea
                  ref={textarea}
                  id={`channel-message-${target}`}
                  className="min-h-16 resize-none border-0 bg-transparent shadow-none focus-visible:outline-none"
                  value={draft.text}
                  maxLength={64000}
                  rows={2}
                  placeholder={
                    thread
                      ? threads.replyMessage
                      : locale === "ja-JP"
                        ? "やりたいことや、続けてほしいことを入力…"
                        : "Describe what you want to do or continue…"
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
                  <ul className="flex flex-wrap gap-1.5 px-2 pb-2">
                    {draft.files.map((entry) => (
                      <li
                        key={entry.key}
                        className="inline-flex h-6 max-w-full items-center gap-1.5 rounded-sm border border-border bg-surface pl-2 text-[11px]"
                      >
                        <FileText aria-hidden className="size-3 text-faint" />
                        <span className="truncate font-mono">
                          {entry.file.name}
                        </span>
                        <button
                          type="button"
                          className="grid size-6 place-items-center text-faint hover:text-foreground disabled:opacity-50"
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
                          <X className="size-3" />
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
                <div className="flex items-center gap-0.5 border-t border-border px-1.5 py-1">
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
                  <Button
                    variant="ghost"
                    size="icon"
                    className={toolButton}
                    type="button"
                    disabled={busy}
                    aria-label={words.attach}
                    title={words.attachmentLimit}
                    onClick={() => fileInput.current?.click()}
                  >
                    <Paperclip />
                  </Button>
                  <Popover open={mentioning} onOpenChange={setMentioning}>
                    <PopoverTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className={toolButton}
                        type="button"
                        disabled={busy}
                        aria-label={words.mention}
                      >
                        <AtSign />
                      </Button>
                    </PopoverTrigger>
                    <PopoverContent className="grid w-64 gap-1 p-1.5">
                      <p className="px-2 py-1 text-[11px] text-faint">
                        {words.mentionHelp}
                      </p>
                      {data.registry
                        .filter((entry) => entry.kind === "agent")
                        .map((entry) => (
                          <button
                            type="button"
                            className="flex h-8 items-center gap-2 rounded-sm px-2 text-left text-[13px] hover:bg-accent"
                            key={`${entry.id}@${entry.version}`}
                            onClick={() => {
                              setMentioning(false);
                              insert(`@${entry.id} `);
                            }}
                          >
                            <Avatar
                              name={agentLabel(data.node.id, entry)}
                              small
                            />
                            <span className="truncate">
                              {agentLabel(data.node.id, entry)}
                            </span>
                          </button>
                        ))}
                    </PopoverContent>
                  </Popover>
                  <span aria-hidden className="mx-1 h-4 w-px bg-border" />
                  <Button
                    variant="ghost"
                    size="icon"
                    className={toolButton}
                    type="button"
                    disabled={busy}
                    aria-label={words.bold}
                    onClick={() => insert("**", "**")}
                  >
                    <Bold />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className={toolButton}
                    type="button"
                    disabled={busy}
                    aria-label={words.italic}
                    onClick={() => insert("_", "_")}
                  >
                    <Italic />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className={toolButton}
                    type="button"
                    disabled={busy}
                    aria-label={words.code}
                    onClick={() => insert("`", "`")}
                  >
                    <Code2 />
                  </Button>
                  <span className="ml-auto hidden items-center gap-1 pr-1 text-[11px] text-faint lg:flex">
                    <Kbd>Enter</Kbd>
                    {locale === "ja-JP" ? "で送信" : "to send"}
                  </span>
                  <Button
                    size="sm"
                    className="ml-auto lg:ml-1"
                    aria-label={thread ? threads.sendReply : copy.send}
                    title={words.enterHint}
                    disabled={
                      busy || (!draft.text.trim() && draft.files.length === 0)
                    }
                  >
                    {sending ? (
                      <span>{uploading ? words.uploading : copy.sending}</span>
                    ) : (
                      <SendHorizontal />
                    )}
                  </Button>
                </div>
              </div>
            </form>
          )}
        </>
      )}
      {sent && (
        <span className="sr-only" role="status">
          {copy.sent}
        </span>
      )}
    </div>
  );
}
