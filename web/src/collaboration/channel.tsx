import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { messageCreate, workspaceGet } from "../generated/aidash";
import type {
  Artifact,
  HumanRequest,
  Run,
  State,
  Task,
  Workspace,
} from "../types";
import { Badge, JsonView, useI18n } from "../ui";
import { collaborationCopy } from "./copy";
import { senderLabel, taskProgress } from "./model";
import { entityKey } from "../agent-graph/model";
import type { Selection } from "./details";

export function ArtifactList({ artifacts }: { artifacts: Artifact[] }) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  return (
    <section className="collab-artifacts" aria-label={copy.results}>
      <h3>{copy.results}</h3>
      {artifacts.length === 0 && <p className="muted">{copy.empty}</p>}
      {artifacts.map((artifact) => (
        <details key={artifact.id}>
          <summary>
            <strong>{artifact.name}</strong> <Badge value={artifact.kind} />
          </summary>
          <JsonView value={artifact.content} />
          <small>{artifact.created_by}</small>
        </details>
      ))}
    </section>
  );
}

export function Channel({
  workspace,
  data,
  runs,
  requests,
  open,
  graph,
}: {
  workspace: Workspace;
  data: State;
  runs: { run: Run; node: string }[];
  requests: { request: HumanRequest; node: string }[];
  open: (selection: Selection) => void;
  graph: (focus?: string) => void;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const client = useQueryClient();
  const [tab, setTab] = useState<"conversation" | "work" | "activity">(
    "conversation",
  );
  const [draft, setDraft] = useState("");
  const [error, setError] = useState("");
  const [sent, setSent] = useState(false);
  const [sending, setSending] = useState(false);
  const submission = useRef<{ content: string; key: string } | null>(null);
  const inFlight = useRef(false);
  const bottom = useRef<HTMLDivElement>(null);
  const scroll = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const [nearBottom, setNearBottom] = useState(true);
  const query = useQuery({
    queryKey: ["workspace", workspace.id],
    queryFn: () => workspaceGet(workspace.id),
    refetchInterval: 2000,
    retry: false,
  });
  const messages = query.isError ? [] : (query.data?.messages ?? []);
  useEffect(() => {
    if (follow.current && tab === "conversation")
      bottom.current?.scrollIntoView({ block: "end" });
  }, [messages.length, tab]);
  const tasks: Task[] = query.isError
    ? []
    : (query.data?.tasks ??
      data.tasks.filter((task) => task.workspace_id === workspace.id));
  const scopedRuns = runs.filter(
    ({ run }) =>
      run.workspace_id === workspace.id && run.home_node === data.node.id,
  );
  const scopedRequests = requests.filter(
    ({ request }) =>
      request.workspace_id === workspace.id && request.response === null,
  );
  const progress = taskProgress(tasks, workspace.id);
  const agents = Array.from(
    new Map(
      scopedRuns.map(({ run, node }) => [
        JSON.stringify([node, run.agent_id, run.agent_version]),
        { node, id: run.agent_id, version: run.agent_version },
      ]),
    ).values(),
  );
  async function send() {
    const content = draft.trim();
    if (!content || inFlight.current || query.isError) return;
    inFlight.current = true;
    setSending(true);
    setError("");
    setSent(false);
    if (submission.current?.content !== content)
      submission.current = { content, key: crypto.randomUUID() };
    try {
      await messageCreate(workspace.id, {
        content,
        idempotency_key: submission.current.key,
      });
      setDraft("");
      submission.current = null;
      setSent(true);
      follow.current = true;
      await client.invalidateQueries({ queryKey: ["workspace", workspace.id] });
    } catch (reason) {
      setError(
        `${copy.failed} ${reason instanceof Error ? reason.message : String(reason)}`,
      );
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  }
  return (
    <section className="collab-channel" aria-label={workspace.title}>
      <header className="collab-channel-heading">
        <div>
          <span className="eyebrow">{copy.channels}</span>
          <h1 aria-label={`# ${workspace.title}`}>
            <span aria-hidden="true"># </span>
            {workspace.title}
          </h1>
        </div>
        <button type="button" onClick={() => graph()}>
          {copy.graphLink}
        </button>
      </header>
      <details className="collab-goal" open>
        <summary>{copy.goal}</summary>
        <p>{workspace.goal}</p>
      </details>
      <div className="collab-tabs" role="group" aria-label={workspace.title}>
        {(["conversation", "work", "activity"] as const).map((value) => (
          <button
            type="button"
            key={value}
            aria-pressed={tab === value}
            onClick={() => setTab(value)}
          >
            {copy[value]}
          </button>
        ))}
      </div>
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
          {scopedRequests.length > 0 && (
            <section className="collab-attention" aria-label={copy.needsInput}>
              <h3>{copy.needsInput}</h3>
              {scopedRequests.map((item) => (
                <button
                  key={`${item.node}:${item.request.id}`}
                  type="button"
                  className="request-card"
                  onClick={() => open({ kind: "human", ...item })}
                >
                  <Badge value={item.request.kind} />
                  <p>{item.request.prompt}</p>
                  <span>{copy.answer}</span>
                </button>
              ))}
            </section>
          )}
          {tab === "conversation" && (
            <>
              <div
                ref={scroll}
                className="collab-messages"
                aria-label={copy.conversation}
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
                {query.isPending && <p role="status">{copy.processing}</p>}
                {!query.isPending && messages.length === 0 && (
                  <p className="collab-empty">{copy.noMessages}</p>
                )}
                {messages.map((message) => {
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
                    </article>
                  );
                })}
                <div ref={bottom} />
              </div>
              {!nearBottom && (
                <button
                  className="collab-latest"
                  type="button"
                  onClick={() => {
                    follow.current = true;
                    setNearBottom(true);
                    bottom.current?.scrollIntoView({ block: "end" });
                  }}
                >
                  {copy.newMessages}
                </button>
              )}
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
                  disabled={sending}
                  onChange={(event) => {
                    setDraft(event.target.value);
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
                    disabled={sending || !draft.trim()}
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
          {tab === "work" && (
            <>
              <div className="collab-progress" aria-label={copy.taskProgress}>
                <span>
                  {copy.completed}: {progress.completed} / {progress.total}
                </span>
                <span>
                  {copy.active}: {progress.active}
                </span>
                <span>
                  {copy.attention}: {progress.attention}
                </span>
              </div>
              <button
                type="button"
                onClick={() => open({ kind: "task", workspace })}
              >
                {copy.newTask}
              </button>
              {tasks.length === 0 && (
                <p className="collab-empty">{copy.noTasks}</p>
              )}
              {tasks.map((task) => (
                <button
                  type="button"
                  className="collab-task"
                  key={task.id}
                  onClick={() => open({ kind: "taskDetail", task })}
                >
                  <Badge value={task.status} />
                  <span>
                    <strong>{task.title}</strong>
                    <small>{task.description}</small>
                  </span>
                </button>
              ))}
              <ArtifactList artifacts={query.data?.artifacts ?? []} />
              <h3>{copy.participants}</h3>
              {agents.map((agent) => (
                <button
                  className="collab-agent"
                  type="button"
                  key={JSON.stringify(agent)}
                  onClick={() => graph(entityKey(agent.node, "agent", agent))}
                >
                  {agent.id} · v{agent.version}
                  <small>{agent.node}</small>
                </button>
              ))}
            </>
          )}
          {tab === "activity" && (
            <>
              <p className="muted">{copy.limitedHistory}</p>
              {scopedRuns.map((item) => (
                <button
                  type="button"
                  className="collab-task"
                  key={`${item.node}:${item.run.id}`}
                  onClick={() => open({ kind: "run", ...item })}
                >
                  <Badge
                    value={
                      item.run.control === "PAUSED" ? "PAUSED" : item.run.phase
                    }
                  />
                  <span>
                    {item.run.agent_id} · v{item.run.agent_version}
                    <small>{item.node}</small>
                  </span>
                </button>
              ))}
              {[...(query.data?.events ?? [])].reverse().map((event) => (
                <details className="call-detail" key={event.id}>
                  <summary>
                    {event.kind} ·{" "}
                    {new Date(event.created_at).toLocaleString(locale)}
                  </summary>
                  <JsonView value={event.data} />
                </details>
              ))}
            </>
          )}
        </>
      )}
    </section>
  );
}
