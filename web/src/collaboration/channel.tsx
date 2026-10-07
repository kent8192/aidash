import { ReferenceName, RecordView } from "../record-view";
import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  CheckCircle2,
  FileText,
  Zap,
  Network,
  Plus,
  PanelRight,
  ChevronDown,
  MoreHorizontal,
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

import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "../components/ui/sheet";
import { ChannelFiles } from "./attachments";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../components/ui/dropdown-menu";

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
  tools,
}: {
  discovery?: Discovery;
  workspace: Workspace;
  data: State;
  runs: { run: Run; node: string }[];
  requests: { request: HumanRequest; node: string }[];
  open: (selection: Selection) => void;
  graph: (focus?: string) => void;
  threadList?: boolean;
  tools: (view: "files" | "progress") => void;
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
    setStatusOpen(value !== "conversation");
    setThread(null);
    if (threadList)
      void navigate({
        to: "/$section",
        params: { section: "collaboration" },
        search: { channel: workspace.id },
      });
  };
  const inspect = (selection: Selection) => {
    setStatusOpen(false);
    open(selection);
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
              <h1 aria-label={`# ${workspace.title}`}>{workspace.title}</h1>
              <span className="workspace-channel-state">{status}</span>
            </div>
            <p>{workspace.goal}</p>
          </div>
          <div className="workspace-heading-actions">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={
                    locale === "ja-JP" ? "依頼の操作" : "Request actions"
                  }
                >
                  <MoreHorizontal />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onSelect={() => {
                    void navigate({
                      to: "/$section",
                      params: { section: "collaboration" },
                      search: {
                        channel: workspace.id,
                        view: threadList ? undefined : "threads",
                      },
                    });
                  }}
                >
                  {threadList ? copy.conversation : words.threads}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onSelect={() => open({ kind: "task", workspace })}
                >
                  {copy.newTask}
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={() => graph()}>
                  {copy.graphLink}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <Button
              variant="ghost"
              aria-label={locale === "ja-JP" ? "ファイル" : "Files"}
              onClick={() => tools("files")}
            >
              <FileText size={16} />
              <span>{locale === "ja-JP" ? "ファイル" : "Files"}</span>
            </Button>
            <Button
              variant="outline"
              aria-label={words.details}
              aria-expanded={statusOpen}
              onClick={() => {
                setTab("conversation");
                setStatusOpen(true);
              }}
            >
              <PanelRight size={16} />
              <span>{locale === "ja-JP" ? "進捗を見る" : "View progress"}</span>
            </Button>
          </div>
        </header>
        {query.isError ? (
          <div className="error" role="alert">
            <p>{copy.unavailable}</p>
            <p>{query.error.message}</p>
            <Button
              variant="outline"
              type="button"
              onClick={() => void query.refetch()}
            >
              {copy.retry}
            </Button>
          </div>
        ) : (
          <>
            <ChannelConversation
              key={workspace.id}
              workspace={workspace.id}
              title={workspace.title}
              data={data}
              discovery={discovery}
              visible={true}
              threadList={threadList}
              thread={thread}
              selectThread={setThread}
              threadContainer={threadContainer}
              progress={
                <div className="intent-context">
                  {tasks.length > 0 && (
                    <Collapsible defaultOpen className="intent-inline-progress">
                      <CollapsibleTrigger asChild>
                        <Button variant="ghost" className="intent-tool-trigger">
                          <span>
                            <CheckCircle2 size={16} />
                            {locale === "ja-JP" ? "進捗" : "Progress"} ·{" "}
                            {progress.completed} / {progress.total}
                          </span>
                          <ChevronDown size={16} />
                        </Button>
                      </CollapsibleTrigger>
                      <CollapsibleContent>
                        {[...tasks]
                          .sort(
                            (a, b) =>
                              Number(a.status === "COMPLETED") -
                              Number(b.status === "COMPLETED"),
                          )
                          .slice(0, 3)
                          .map((task) => (
                            <Button
                              variant="outline"
                              type="button"
                              className="intent-task-row"
                              key={task.id}
                              onClick={() => open({ kind: "taskDetail", task })}
                            >
                              <Badge value={task.status} />
                              <span>{task.title}</span>
                              <small>
                                {task.owner ? (
                                  <ReferenceName id={task.owner} />
                                ) : (
                                  copy.noAgent
                                )}
                              </small>
                            </Button>
                          ))}
                      </CollapsibleContent>
                    </Collapsible>
                  )}
                  <div className="intent-context-actions">
                    <Button
                      variant="ghost"
                      onClick={() => selectTab("work")}
                      aria-label={copy.work}
                    >
                      <CheckCircle2 size={15} />
                      {locale === "ja-JP"
                        ? "担当とタスクを見る"
                        : "Tasks and owners"}
                    </Button>
                    <Button
                      variant="ghost"
                      onClick={() => selectTab("activity")}
                      aria-label={copy.activity}
                    >
                      <Zap size={15} />
                      {copy.activity}
                    </Button>
                    <Button
                      variant="ghost"
                      onClick={() => graph()}
                      aria-label={copy.graphLink}
                    >
                      <Network size={15} />
                      {copy.graphLink}
                    </Button>
                  </div>
                  {artifacts.length > 0 && (
                    <ArtifactList artifacts={artifacts} />
                  )}
                </div>
              }
              requests={
                <ChannelRequests
                  requests={scopedRequests}
                  localNode={data.node.id}
                  open={open}
                />
              }
            />
          </>
        )}
      </div>
      <div
        ref={setThreadContainer}
        id="workspace-thread-panel"
        className="workspace-thread-panel"
        hidden={!thread || query.isError || shownTab !== "conversation"}
      />
      <Sheet open={statusOpen} onOpenChange={setStatusOpen}>
        <SheetContent
          className="intent-progress-sheet collab-channel"
          data-theme={document.documentElement.dataset.theme}
          closeLabel={locale === "ja-JP" ? "閉じる" : "Close"}
        >
          <SheetHeader>
            <SheetTitle>
              {locale === "ja-JP" ? "依頼の進捗" : "Request progress"}
            </SheetTitle>
            <SheetDescription>{workspace.title}</SheetDescription>
          </SheetHeader>
          <nav className="intent-progress-nav" aria-label={workspace.title}>
            <Button
              variant="ghost"
              onClick={() => setTab("conversation")}
              aria-pressed={tab === "conversation"}
            >
              {locale === "ja-JP" ? "概要" : "Overview"}
            </Button>
            <Button
              variant="ghost"
              onClick={() => setTab("work")}
              aria-pressed={tab === "work"}
            >
              {copy.work}
            </Button>
            <Button
              variant="ghost"
              onClick={() => setTab("results")}
              aria-pressed={tab === "results"}
            >
              {copy.results}
            </Button>
            <Button
              variant="ghost"
              onClick={() => setTab("activity")}
              aria-pressed={tab === "activity"}
            >
              {copy.activity}
            </Button>
            <Button
              variant="ghost"
              onClick={() => {
                setStatusOpen(false);
                tools("progress");
              }}
            >
              {locale === "ja-JP" ? "整合性と復旧" : "Consistency and recovery"}
            </Button>
          </nav>
          {tab === "conversation" && (
            <ChannelStatus
              workspace={workspace}
              data={data}
              tasks={tasks}
              runs={scopedRuns}
              waiting={scopedRequests.length}
              discovery={discovery}
              open={inspect}
              graph={() => graph()}
              showTasks={() => setTab("work")}
              expanded={true}
              close={() => setStatusOpen(false)}
            />
          )}
          {tab === "work" && (
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
              <Button
                variant="outline"
                type="button"
                onClick={() => inspect({ kind: "task", workspace })}
              >
                <Plus size={14} />
                {copy.newTask}
              </Button>
              {tasks.length === 0 && (
                <p className="collab-empty">{copy.noTasks}</p>
              )}
              {tasks.map((task) => (
                <Button
                  variant="outline"
                  type="button"
                  className="collab-task"
                  key={task.id}
                  onClick={() => inspect({ kind: "taskDetail", task })}
                >
                  <Badge value={task.status} />
                  <span>
                    <strong>{task.title}</strong>
                    <small>{task.description}</small>
                  </span>
                </Button>
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
                  <Button
                    variant="outline"
                    className="collab-agent"
                    type="button"
                    key={`${item.node}:${JSON.stringify(ref)}`}
                    onClick={() => graph(entityKey(item.node, "agent", ref))}
                  >
                    {label}
                  </Button>
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
          {tab === "results" && (
            <div className="workspace-tab-content">
              <ArtifactList artifacts={artifacts} />
              <ChannelFiles workspace={workspace.id} />
            </div>
          )}
          {tab === "activity" && (
            <div className="workspace-tab-content">
              <p className="muted">{copy.limitedHistory}</p>
              {scopedRuns.map((item) => (
                <Button
                  variant="outline"
                  type="button"
                  className="collab-task"
                  key={`${item.node}:${item.run.id}`}
                  onClick={() => inspect({ kind: "run", ...item })}
                >
                  <Badge
                    value={
                      item.run.control === "PAUSED" ? "PAUSED" : item.run.phase
                    }
                  />
                  <span>
                    {participantLabel(item)}
                    <small>
                      <ReferenceName id={item.node} />
                    </small>
                  </span>
                </Button>
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
        </SheetContent>
      </Sheet>
    </section>
  );
}
