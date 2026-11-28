import { Button } from "../components/ui/button";
import { ThreadApprovalMode } from "../thread-approval-mode";
import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Alert, Loading, Notice } from "../components/patterns";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDown,
  AtSign,
  Bold,
  Code2,
  FileText,
  Italic,
  MessageSquare,
  Paperclip,
  SendHorizontal,
  X,
} from "lucide-react";
import {
  channelAttachmentDescription,
  channelAttachmentUpload,
  channelMessageCreate,
  channelMessageHistory,
} from "../generated/aidash";
import type { ChannelAttachment, ChannelCitation } from "../generated/models";
import { AttachmentCard } from "./attachments";
import type { State, Discovery } from "../types";
import { useI18n, useAgentLabel } from "../ui";
import { cn } from "../lib/utils";
import { readUnconfirmed, unconfirmedFor } from "../unconfirmed";
import { UnconfirmedNotice } from "../unconfirmed-notice";
import { collaborationCopy } from "./copy";
import { threadCopy } from "./thread-copy";
import { senderLabel } from "./model";
import {
  mergeMessagePages,
  attachmentWarningKey,
  attachmentWarningText,
  submissionFor,
  descriptionTooLong,
  uploadDescription,
  validAttachments,
  type MessageSubmission,
} from "./conversation-model";
import { Avatar } from "./avatar";
import { messageParts } from "./message-text-model";
import { Textarea } from "../components/ui/textarea";
import { Kbd } from "../components/ui/kbd";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/popover";

type DraftFile = {
  key: string;
  file: File;
  uploaded?: ChannelAttachment;
  /** The sender's Image Description of an image, sent with its upload. */
  description?: string;
};
type Draft = { text: string; files: DraftFile[] };
type Drafts = Record<string, Draft>;
const emptyDraft: Draft = { text: "", files: [] };

/** A thread event shown inline in the channel feed as a muted system line. */
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
      {messageParts(text).map((part, i) => {
        switch (part.kind) {
          case "strong":
            return (
              <strong key={i} className="font-semibold">
                {part.text}
              </strong>
            );
          case "em":
            return <em key={i}>{part.text}</em>;
          case "code":
            return (
              <code
                key={i}
                className="rounded-sm bg-raised px-1 py-px font-mono text-[12px]"
              >
                {part.text}
              </code>
            );
          case "mention":
            return (
              <span className="font-medium text-brand" key={i}>
                {part.text}
              </span>
            );
          default:
            return <Fragment key={i}>{part.text}</Fragment>;
        }
      })}
    </>
  );
}

/** Cited File versions under a message. Unavailable ones never become links. */
function MessageCitations({ citations }: { citations: ChannelCitation[] }) {
  const { t } = useI18n();
  if (citations.length === 0) return null;
  const reason = (value: string | null | undefined) =>
    value === "forbidden"
      ? t("fileCitationForbidden")
      : value === "released"
        ? t("fileCitationReleased")
        : t("fileCitationUnavailable");
  return (
    <ul
      aria-label={t("fileCitationMessageChips")}
      className="mt-1.5 flex flex-wrap gap-1.5"
    >
      {citations.map((citation) => {
        const label = (
          <>
            <FileText aria-hidden className="size-3 shrink-0" />
            <span className="truncate font-mono">{citation.path}</span>
            {citation.digest && (
              <span className="shrink-0 font-mono text-faint">
                {citation.digest.slice(0, 8)}
              </span>
            )}
          </>
        );
        return (
          <li
            key={`${citation.area_id}:${citation.file_id}`}
            className="min-w-0"
          >
            <span
              className="inline-flex h-6 max-w-full items-center gap-1.5 rounded-sm border border-border bg-surface px-2 text-[11px] text-faint"
              title={citation.path}
            >
              {label}
              <span className="shrink-0">· {reason(citation.reason)}</span>
            </span>
          </li>
        );
      })}
    </ul>
  );
}

type ConversationProps = {
  data: State;
  discovery?: Discovery;
  thread: string;
  title: string;
  visible: boolean;
  /** Rendered after the channel messages (pending decisions on narrow screens). */
  requests?: ReactNode;
  system?: SystemLine[];
};
export function ChannelConversation(props: ConversationProps) {
  const [drafts, setDrafts] = useState<Drafts>({});
  const [pending] = useState(() => new Map<string, MessageSubmission>());
  return (
    <ConversationFeed
      {...props}
      drafts={drafts}
      setDrafts={setDrafts}
      pending={pending}
    />
  );
}

const toolButton =
  "size-7 text-muted-foreground hover:text-foreground [&_svg]:size-3.5";

function ConversationFeed({
  thread,
  visible,
  data,
  discovery,
  requests,
  system,
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
  const client = useQueryClient();
  const copy = collaborationCopy[locale],
    words = threadCopy[locale];
  const target = "channel",
    draft = drafts[target] ?? emptyDraft;
  // A person sets their Approval Mode for the Thread beside the composer.
  const approvalTenant =
    data.access.kind === "subject" ? data.access.tenant : null;
  const updateDraft = (update: (draft: Draft) => Draft) =>
    setDrafts((current) => ({
      ...current,
      [target]: update(current[target] ?? emptyDraft),
    }));
  const inFlight = useRef(false);
  const warnedAttachments = useRef<string | null>(null);
  const [sending, setSending] = useState(false),
    [uploading, setUploading] = useState(false),
    [mentioning, setMentioning] = useState(false);
  const [error, setError] = useState(""),
    [sent, setSent] = useState(false);
  const [readabilityNotice, setReadabilityNotice] = useState(false);
  const scroll = useRef<HTMLDivElement>(null),
    latest = useRef<HTMLElement>(null),
    textarea = useRef<HTMLTextAreaElement>(null),
    fileInput = useRef<HTMLInputElement>(null);
  const follow = useRef(true);
  const [nearBottom, setNearBottom] = useState(true);
  const query = useInfiniteQuery({
    queryKey: ["channel-history", thread],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      channelMessageHistory(
        thread,
        { before: pageParam, limit: 40 },
        { signal },
      ),
    getNextPageParam: (page) => page.next_before ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const allMessages = query.isError
    ? []
    : mergeMessagePages(query.data?.pages ?? []);
  // Every loaded page is a read of its own; what each left out adds up.
  const missing = query.isError
    ? []
    : unconfirmedFor(
        (query.data?.pages ?? []).flatMap(readUnconfirmed),
        ["message"],
        [thread],
      );
  const messages = allMessages;
  const latestMessageId = messages.at(-1)?.message.id;
  const scrollToLatest = useCallback(() => {
    const element = scroll.current;
    if (element) element.scrollTop = latestMessageTop(element, latest.current);
  }, []);
  useEffect(() => {
    if (visible && follow.current) scrollToLatest();
  }, [latestMessageId, visible, scrollToLatest]);

  async function send() {
    if (
      (!draft.text.trim() && draft.files.length === 0) ||
      inFlight.current ||
      query.isError ||
      query.isPending
    )
      return;
    if (
      draft.files.some(
        (entry) =>
          entry.file.type.startsWith("image/") &&
          descriptionTooLong(entry.description),
      )
    ) {
      setError(words.imageDescriptionTooLong);
      return;
    }
    inFlight.current = true;
    setSending(true);
    setError("");
    setReadabilityNotice(false);
    setSent(false);
    try {
      const attachmentIds: string[] = [];
      const uploadedFiles: ChannelAttachment[] = [];
      for (const entry of draft.files) {
        let uploaded = entry.uploaded;
        if (!uploaded) {
          setUploading(true);
          uploaded = await channelAttachmentUpload(thread, entry.file, {
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
        // The description travels in a JSON body, never in the upload URL.
        const description = uploadDescription(
          entry.file.type,
          entry.description,
        );
        if (description && uploaded.description !== description) {
          uploaded = await channelAttachmentDescription(thread, uploaded.id, {
            description,
          });
          const described = uploaded;
          updateDraft((current) => ({
            ...current,
            files: current.files.map((file) =>
              file.key === entry.key ? { ...file, uploaded: described } : file,
            ),
          }));
        }
        attachmentIds.push(uploaded.id);
        uploadedFiles.push(uploaded);
      }
      setUploading(false);
      const warningKey = attachmentWarningKey(uploadedFiles);
      if (warningKey && warnedAttachments.current !== warningKey) {
        warnedAttachments.current = warningKey;
        setReadabilityNotice(true);
        return;
      }
      const submission = submissionFor(
        pending.get(target) ?? null,
        thread,
        draft.text,
        () => crypto.randomUUID(),
        attachmentIds,
      );
      pending.set(target, submission);
      await channelMessageCreate(thread, {
        content: submission.content,
        idempotency_key: submission.key,
        attachment_ids: submission.attachments,
      });
      updateDraft(() => ({ text: "", files: [] }));
      warnedAttachments.current = null;
      pending.delete(target);
      setSent(true);
      follow.current = true;
      await Promise.all([
        client.invalidateQueries({ queryKey: ["channel-history", thread] }),
        client.invalidateQueries({ queryKey: ["thread", thread] }),
      ]);
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(`${copy.failed} ${message}`);
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
    setReadabilityNotice(false);
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
  const busy = sending || query.isPending;
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
  const systemLines = firstShown
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
            aria-label={copy.conversation}
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
            <UnconfirmedNotice className="mb-2" lines={missing} />
            {query.hasNextPage && (
              <Button
                variant="ghost"
                size="sm"
                className="mx-auto mb-2 flex text-xs"
                type="button"
                disabled={query.isFetchingNextPage}
                onClick={() => void loadOlder()}
              >
                {copy.older}
              </Button>
            )}
            {query.isPending && (
              <Loading className="py-6 text-center">{copy.processing}</Loading>
            )}
            {!query.isPending &&
              messages.length === 0 &&
              missing.length === 0 && (
                <div className="mx-auto mt-8 grid max-w-64 justify-items-center gap-2 text-center">
                  <span className="grid size-8 place-items-center rounded-md bg-raised text-faint">
                    <MessageSquare aria-hidden className="size-4" />
                  </span>
                  <p className="text-[13px] font-medium text-foreground">
                    {copy.noMessages}
                  </p>
                  <p className="text-xs text-muted-foreground">{words.scope}</p>
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
                    id={`message-${message.id}`}
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
                      {message.completion && (
                        <div className="mt-2 grid gap-1 rounded-md border border-border px-3 py-2 text-xs">
                          <strong>
                            {locale === "ja-JP"
                              ? "完了メッセージ"
                              : "Completion Message"}
                          </strong>
                          {message.completion.verdict && (
                            <p>
                              {message.completion.verdict.verdict} ·{" "}
                              {message.completion.verdict.reason}
                            </p>
                          )}
                          {message.completion.deliverable.drive_versions.map(
                            (version) => (
                              <p key={`${version.drive_id}:${version.version}`}>
                                Drive v{version.version} ·{" "}
                                {version.paths.join(", ")}
                              </p>
                            ),
                          )}
                          {message.completion.deliverable.outside_versions.map(
                            (version, index) => (
                              <p key={index}>
                                {version.system} · {version.item} ·{" "}
                                {version.version}
                                {version.url ? ` · ${version.url}` : ""}
                              </p>
                            ),
                          )}
                        </div>
                      )}
                      {entry.citations && entry.citations.length > 0 && (
                        <MessageCitations citations={entry.citations} />
                      )}
                      {entry.attachments.length > 0 && (
                        <ul className="mt-1.5 flex flex-wrap gap-1.5">
                          {entry.attachments.map((attachment) => (
                            <li key={attachment.id} className="min-w-0">
                              <AttachmentCard
                                thread={thread}
                                attachment={attachment}
                              />
                            </li>
                          ))}
                        </ul>
                      )}
                    </div>
                  </article>
                </Fragment>
              );
            })}
            {requests}
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
          {error && <Alert className="mx-4 mb-2">{error}</Alert>}
          {readabilityNotice && (
            <Notice tone="warning" className="mx-4 mb-2" role="status">
              {words.attachmentSendAnyway}
            </Notice>
          )}
          {
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
                  {copy.message}
                </label>
                <Textarea
                  ref={textarea}
                  id={`channel-message-${target}`}
                  className="min-h-16 resize-none border-0 bg-transparent shadow-none focus-visible:outline-none"
                  value={draft.text}
                  maxLength={64000}
                  rows={2}
                  placeholder={
                    locale === "ja-JP"
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
                      <li key={entry.key} className="max-w-full">
                        <span className="inline-flex h-6 max-w-full items-center gap-1.5 rounded-sm border border-border bg-surface pl-2 text-[11px]">
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
                        </span>
                        {entry.file.type.startsWith("image/") && (
                          <input
                            className="mt-1 block h-6 w-full max-w-sm rounded-sm border border-border bg-background px-2 text-[11px]"
                            aria-invalid={descriptionTooLong(entry.description)}
                            disabled={busy}
                            value={entry.description ?? ""}
                            placeholder={words.imageDescriptionPlaceholder}
                            aria-label={`${words.imageDescription}: ${entry.file.name}`}
                            onChange={(event) =>
                              updateDraft((current) => ({
                                ...current,
                                files: current.files.map((file) =>
                                  file.key === entry.key
                                    ? {
                                        ...file,
                                        description: event.target.value,
                                      }
                                    : file,
                                ),
                              }))
                            }
                          />
                        )}
                        {entry.file.type.startsWith("image/") &&
                          descriptionTooLong(entry.description) && (
                            <p
                              className="text-xs text-destructive"
                              role="alert"
                            >
                              {words.imageDescriptionTooLong}
                            </p>
                          )}
                        {entry.uploaded?.no_assigned_agent_can_read && (
                          <p className="text-xs text-warning">
                            {attachmentWarningText(
                              entry.uploaded.media_type,
                              words,
                            )}
                          </p>
                        )}
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
                  {approvalTenant !== null && (
                    <ThreadApprovalMode thread={thread} />
                  )}
                  {approvalTenant === null && (
                    <span className="ml-auto hidden items-center gap-1 pr-1 text-[11px] text-faint lg:flex">
                      <Kbd>Enter</Kbd>
                      {locale === "ja-JP" ? "で送信" : "to send"}
                    </span>
                  )}
                  <Button
                    size="sm"
                    className={cn(
                      "ml-auto",
                      approvalTenant === null && "lg:ml-1",
                    )}
                    aria-label={copy.send}
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
          }
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
