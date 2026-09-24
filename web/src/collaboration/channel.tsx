import { ReferenceName, RecordView } from "../record-view";
import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  MessageSquare,
  CheckCircle2,
  FileText,
  Zap,
  Network,
  ArrowUpRight,
  Target,
  ChevronRight,
  Plus,
  PanelRight,
  Hash,
} from "lucide-react";
import { workspaceGet } from "../generated/aidash";
import type {
  Artifact,
  Discovery,
  HumanRequest,
  Run,
  State,
  Task,
  Workspace,
} from "../types";
import { Badge, JsonView, useI18n, useAgentLabel } from "../ui";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";
import { taskProgress } from "./model";
import { entityKey } from "../agent-graph/model";
import type { Selection } from "./details";
import { ChannelConversation } from "./conversation";
import { ChannelStatus } from "./status";
import { channelAgents, terminalRun } from "./workspace-model";
import { ChannelRequests } from "./requests";
import { Avatar } from "./avatar";
import { ChannelFiles } from "./attachments";

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
            <FileText size={16} />
            <strong>{artifact.name}</strong>
            <Badge value={artifact.kind} />
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
  discovery,
  runs,
  requests,
  open,
  graph,
  threadList = false,
}: {
  discovery?: Discovery;
  workspace: Workspace;
  data: State;
  runs: { run: Run; node: string }[];
  requests: { request: HumanRequest; node: string }[];
  open: (selection: Selection) => void;
  graph: (focus?: string) => void;
  threadList?: boolean;
}) {
  const { locale } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale];
  const words = workspaceCopy[locale];
  const navigate = useNavigate();
  const [thread, setThread] = useState<string | null>(null);
  const [threadContainer, setThreadContainer] = useState<HTMLDivElement | null>(
    null,
  );
  const [tab, setTab] = useState<
    "conversation" | "work" | "results" | "activity"
  >("conversation");
  const [statusOpen, setStatusOpen] = useState(false);
  const query = useQuery({
    queryKey: ["workspace", workspace.id],
    queryFn: ({ signal }) => workspaceGet(workspace.id, { signal }),
    refetchInterval: 2000,
    retry: false,
  });
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
  const agents = channelAgents(scopedRuns);
  const progress = taskProgress(tasks, workspace.id);
  const active = scopedRuns.filter((item) => !terminalRun(item.run));
  const status = active.length
    ? active.every((item) => item.run.control === "PAUSED")
      ? words.paused
      : words.active
    : scopedRuns.length &&
        scopedRuns.every((item) => item.run.phase === "COMPLETED")
      ? words.completed
      : words.preparing;
  const artifacts = query.isError ? [] : (query.data?.artifacts ?? []);
  const participantLabel = (item: (typeof agents)[number]) =>
    agentLabel(item.node, {
      id: item.run.agent_id,
      version: item.run.agent_version,
    });
  const shownTab = threadList ? "conversation" : tab;
  const selectTab = (value: typeof tab) => {
    setTab(value);
    setThread(null);
    if (threadList)
      void navigate({
        to: "/$section",
        params: { section: "collaboration" },
        search: { channel: workspace.id },
      });
  };
  return (
    <section
      className={`collab-channel ${thread ? "has-thread" : ""}`}
      aria-label={workspace.title}
    >
      <div className="workspace-channel-center">
        <header className="collab-channel-heading">
          <div>
            <div className="workspace-title-line">
              <Hash size={23} />
              <h1 aria-label={`# ${workspace.title}`}>{workspace.title}</h1>
              <span className="workspace-channel-state">{status}</span>
            </div>
            <p>{workspace.goal}</p>
          </div>
          <div className="workspace-heading-actions">
            <div
              className="workspace-avatar-stack"
              aria-label={`${words.participants}: ${agents.length}`}
            >
              {agents.slice(0, 4).map((item) => (
                <Avatar
                  key={`${item.node}:${item.run.agent_id}:${item.run.agent_version}`}
                  name={participantLabel(item)}
                  small
                />
              ))}
              <small>{agents.length}</small>
            </div>
            <button
              type="button"
              onClick={() => open({ kind: "task", workspace })}
            >
              <Plus size={14} />
              <span>{words.manage}</span>
            </button>
            <button
              type="button"
              className="workspace-status-toggle"
              aria-label={words.details}
              aria-expanded={statusOpen}
              onClick={() => setStatusOpen((value) => !value)}
            >
              <PanelRight size={17} />
            </button>
          </div>
        </header>
        <div className="workspace-tabbar">
          <div
            className="collab-tabs"
            role="group"
            aria-label={workspace.title}
          >
            {(
              [
                ["conversation", MessageSquare, words.messages, undefined],
                ["work", CheckCircle2, words.task, tasks.length],
                ["results", FileText, copy.results, artifacts.length],
                ["activity", Zap, words.events, undefined],
              ] as const
            ).map(([value, Icon, title, count]) => (
              <button
                type="button"
                key={value}
                aria-label={
                  value === "work"
                    ? copy.work
                    : value === "activity"
                      ? copy.activity
                      : title
                }
                aria-pressed={shownTab === value}
                onClick={() => selectTab(value)}
              >
                <Icon size={13} />
                {title}
                {count !== undefined && (
                  <span className="workspace-count">{count}</span>
                )}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="workspace-graph-link"
            onClick={() => graph()}
            aria-label={copy.graphLink}
          >
            <Network size={13} />
            <span>{copy.graphLink}</span>
            <ArrowUpRight size={12} />
          </button>
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
            <details className="collab-goal">
              <summary>
                <Target size={16} />
                <span>
                  <small>{words.sharedGoal}</small>
                  <span>{workspace.goal}</span>
                </span>
                <ChevronRight size={14} />
              </summary>
              <p>{workspace.goal}</p>
            </details>
            <ChannelConversation
              key={workspace.id}
              workspace={workspace.id}
              title={workspace.title}
              data={data}
              discovery={discovery}
              visible={shownTab === "conversation"}
              threadList={threadList}
              thread={thread}
              selectThread={setThread}
              threadContainer={threadContainer}
              requests={
                <ChannelRequests
                  requests={scopedRequests}
                  localNode={data.node.id}
                  open={open}
                />
              }
            />
            {shownTab === "work" && (
              <div className="workspace-tab-content">
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
                  <Plus size={14} />
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
                <h3>{copy.participants}</h3>
                {agents.map((item) => {
                  const ref = {
                    id: item.run.agent_id,
                    version: item.run.agent_version,
                  };
                  const label = (
                    <>
                      {participantLabel(item)}
                      <small>
                        <ReferenceName id={item.node} />
                      </small>
                    </>
                  );
                  return item.node === data.node.id ? (
                    <button
                      className="collab-agent"
                      type="button"
                      key={`${item.node}:${JSON.stringify(ref)}`}
                      onClick={() => graph(entityKey(item.node, "agent", ref))}
                    >
                      {label}
                    </button>
                  ) : (
                    <div
                      className="collab-agent"
                      key={`${item.node}:${JSON.stringify(ref)}`}
                    >
                      {label}
                    </div>
                  );
                })}
              </div>
            )}
            {shownTab === "results" && (
              <div className="workspace-tab-content">
                <ArtifactList artifacts={artifacts} />
                <ChannelFiles workspace={workspace.id} />
              </div>
            )}
            {shownTab === "activity" && (
              <div className="workspace-tab-content">
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
                        item.run.control === "PAUSED"
                          ? "PAUSED"
                          : item.run.phase
                      }
                    />
                    <span>
                      {participantLabel(item)}
                      <small>
                        <ReferenceName id={item.node} />
                      </small>
                    </span>
                  </button>
                ))}
                {[...(query.data?.events ?? [])].reverse().map((event) => (
                  <details className="call-detail" key={event.id}>
                    <summary>
                      {event.kind} ·{" "}
                      {new Date(event.created_at).toLocaleString(locale)}
                    </summary>
                    <RecordView value={event.data} />
                  </details>
                ))}
              </div>
            )}
          </>
        )}
      </div>
      <div
        ref={setThreadContainer}
        id="workspace-thread-panel"
        className="workspace-thread-panel"
        hidden={!thread || query.isError || shownTab !== "conversation"}
      />
      {!query.isError && !thread && (
        <ChannelStatus
          workspace={workspace}
          data={data}
          tasks={tasks}
          runs={scopedRuns}
          waiting={scopedRequests.length}
          discovery={discovery}
          open={open}
          graph={() => graph()}
          showTasks={() => selectTab("work")}
          expanded={statusOpen}
          close={() => setStatusOpen(false)}
        />
      )}
    </section>
  );
}
