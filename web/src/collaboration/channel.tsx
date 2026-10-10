import { ReferenceName, RecordView } from "../record-view";
import { useState, useSyncExternalStore } from "react";
import { Alert } from "../components/patterns";
import { useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  ChevronRight,
  FileText,
  MoreHorizontal,
  Network,
  PanelRight,
  PanelRightClose,
  PanelRightOpen,
  Pause,
  Play,
  Plus,
} from "lucide-react";
import { workspaceGet } from "../generated/aidash";
import type {
  Artifact,
  Discovery,
  HumanRequest,
  MeshEvent,
  Run,
  State,
  Task,
  Workspace,
} from "../types";
import { Badge, JsonView, useI18n, useAgentLabel } from "../ui";
import { cn } from "../lib/utils";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";
import { threadCopy } from "./thread-copy";
import { taskProgress } from "./model";
import { entityKey } from "../agent-graph/model";
import type { Selection } from "./details";
import { ChannelConversation, type SystemLine } from "./conversation";
import { ChannelStatus, useRunControl } from "./status";
import { channelAgents, terminalRun } from "./workspace-model";
import { ChannelRequests } from "./requests";
import { ChannelFiles } from "./attachments";
import { RequestCanvas } from "./request/canvas";
import { TimelineStrip } from "./request/timeline";
import { Phase, SegmentedProgress } from "./request/progress";
import { parseOwner, runDisplayStatus } from "./request/canvas-model";
import { Button } from "../components/ui/button";
import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from "../components/ui/tabs";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "../components/ui/sheet";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../components/ui/dropdown-menu";

const requestCopy = {
  "ja-JP": {
    actions: "依頼の操作",
    files: "ファイル",
    viewProgress: "進捗を見る",
    progressTitle: "依頼の進捗",
    overview: "概要",
    recovery: "整合性と復旧",
    details: "依頼の詳細",
    conversation: "会話",
    canvas: "キャンバス",
    tasks: "タスク",
    results: "成果物",
    hideDock: "パネルを閉じる",
    showDock: "会話を開く",
    running: "実行中",
    blocked: "保留・要対応",
    done: "完了",
    queued: "未着手",
    since: "開始",
    events: {
      "task.created": "タスクを作成しました",
      "task.claimed": "タスクを引き受けました",
      "task.delegated": "タスクを委任しました",
      "task.completed": "タスクを完了しました",
      "task.abandoned": "タスクを中止しました",
      "run.created": "実行を開始しました",
      "run.cancelled": "実行を中止しました",
    } as Record<string, string>,
  },
  "en-US": {
    actions: "Request actions",
    files: "Files",
    viewProgress: "View progress",
    progressTitle: "Request progress",
    overview: "Overview",
    recovery: "Consistency and recovery",
    details: "Request details",
    conversation: "Conversation",
    canvas: "Canvas",
    tasks: "Tasks",
    results: "Results",
    hideDock: "Hide panel",
    showDock: "Show conversation",
    running: "Running",
    blocked: "Needs attention",
    done: "Done",
    queued: "Not started",
    since: "Started",
    events: {
      "task.created": "Task created",
      "task.claimed": "Task claimed",
      "task.delegated": "Task delegated",
      "task.completed": "Task completed",
      "task.abandoned": "Task abandoned",
      "run.created": "Run started",
      "run.cancelled": "Run cancelled",
    } as Record<string, string>,
  },
} as const;

const desktopQuery = "(min-width: 768px)";
function useDesktop() {
  return useSyncExternalStore(
    (notify) => {
      const media = window.matchMedia(desktopQuery);
      media.addEventListener("change", notify);
      return () => media.removeEventListener("change", notify);
    },
    () => window.matchMedia(desktopQuery).matches,
  );
}

/** Task referenced by a workspace event payload (`Task`, `{ task }` or `Run`). */
function eventTask(event: MeshEvent, tasks: readonly Task[]) {
  const data = event.data as {
    id?: unknown;
    task_id?: unknown;
    task?: { id?: unknown };
  } | null;
  const id = event.kind.startsWith("task.")
    ? (data?.task?.id ?? data?.id)
    : data?.task_id;
  return tasks.find((task) => task.id === id);
}

export function ArtifactList({ artifacts }: { artifacts: Artifact[] }) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  return (
    <section className="grid gap-2" aria-label={copy.results}>
      <h3 className="text-[11px] font-medium text-faint">{copy.results}</h3>
      {artifacts.length === 0 && (
        <p className="text-xs text-faint">{copy.empty}</p>
      )}
      {artifacts.length > 0 && (
        <ul className="grid divide-y divide-border rounded-md border border-border">
          {artifacts.map((artifact) => (
            <li key={artifact.id}>
              <details className="group">
                <summary className="flex cursor-pointer list-none items-center gap-2.5 px-3 py-2 transition-colors hover:bg-accent [&::-webkit-details-marker]:hidden">
                  <span className="grid size-7 shrink-0 place-items-center rounded-md bg-raised text-muted-foreground">
                    <FileText aria-hidden className="size-3.5" />
                  </span>
                  <span className="grid min-w-0 flex-1">
                    <strong className="truncate font-mono text-xs font-medium text-foreground">
                      {artifact.name}
                    </strong>
                    <span className="truncate text-[11px] text-faint">
                      <ReferenceName id={artifact.created_by} /> ·{" "}
                      <span className="font-mono tabular">
                        {new Date(artifact.created_at).toLocaleString(locale)}
                      </span>
                    </span>
                  </span>
                  <Badge value={artifact.kind} />
                  <ChevronRight
                    aria-hidden
                    className="size-3.5 shrink-0 text-faint transition-transform group-open:rotate-90"
                  />
                </summary>
                <div className="px-3 pb-3">
                  <JsonView value={artifact.content} />
                </div>
              </details>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

const channelState = {
  active: "RUNNING",
  paused: "PAUSED",
  completed: "COMPLETED",
  preparing: "OPEN",
} as const;

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
  const { locale, t } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale];
  const words = workspaceCopy[locale];
  const view = requestCopy[locale];
  const navigate = useNavigate();
  const desktop = useDesktop();
  const [thread, setThread] = useState<string | null>(null);
  const [threadContainer, setThreadContainer] = useState<HTMLElement | null>(
    null,
  );
  const [tab, setTab] = useState<
    "conversation" | "canvas" | "tasks" | "results"
  >("conversation");
  const [dockOpen, setDockOpen] = useState(true);
  const [sheetTab, setSheetTab] = useState<
    "overview" | "work" | "results" | "activity"
  >("overview");
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
  const control = useRunControl(scopedRuns, data.node.id);
  const active = scopedRuns.filter((item) => !terminalRun(item.run));
  const state = active.length
    ? active.every((item) => item.run.control === "PAUSED")
      ? "paused"
      : "active"
    : scopedRuns.length &&
        scopedRuns.every((item) => item.run.phase === "COMPLETED")
      ? "completed"
      : "preparing";
  const artifacts = query.isError ? [] : (query.data?.artifacts ?? []);
  const events = query.isError ? [] : (query.data?.events ?? []);
  // The workspace snapshot refreshes every 2 s; its fetch time is the render's "now".
  const now = query.dataUpdatedAt;
  const system: SystemLine[] = events.flatMap((event) => {
    const label = view.events[event.kind];
    const task = label && eventTask(event, tasks);
    return label
      ? [
          {
            id: event.id,
            created_at: event.created_at,
            text: task ? `${label}: ${task.title}` : label,
          },
        ]
      : [];
  });
  const participantLabel = (item: (typeof agents)[number]) =>
    agentLabel(item.node, {
      id: item.run.agent_id,
      version: item.run.agent_version,
    });
  const ownerLabel = (task: Task) => {
    const owner = parseOwner(task.owner);
    return owner ? agentLabel(owner.node, owner.agent) : undefined;
  };
  const showSheet = (value: typeof sheetTab) => {
    setSheetTab(value);
    setStatusOpen(true);
  };
  const inspect = (selection: Selection) => {
    setStatusOpen(false);
    open(selection);
  };
  const shownTab = desktop && tab === "canvas" ? "conversation" : tab;
  const unavailable = query.isError && (
    <Alert
      className="mx-5 mt-3"
      retry={() => void query.refetch()}
      retryLabel={copy.retry}
    >
      <p className="font-medium">{copy.unavailable}</p>
      <p className="break-words text-muted-foreground">{query.error.message}</p>
    </Alert>
  );
  const decisions = (
    <ChannelRequests
      className="mt-3"
      requests={scopedRequests}
      localNode={data.node.id}
      open={open}
    />
  );
  const canvas = (
    <>
      <RequestCanvas
        data={data}
        discovery={discovery}
        tasks={tasks}
        runs={scopedRuns}
        requests={desktop ? scopedRequests : []}
        open={open}
        addTask={() => open({ kind: "task", workspace })}
      />
      {query.data && (
        <TimelineStrip
          events={events}
          now={now}
          history={() => showSheet("activity")}
        />
      )}
    </>
  );
  const groups = [
    { label: view.running, statuses: ["RUNNING", "CLAIMED"] },
    { label: view.blocked, statuses: ["BLOCKED", "FAILED"] },
    { label: view.queued, statuses: ["OPEN"] },
    {
      label: view.done,
      statuses: ["COMPLETED", "CANCELLED", "ABANDONED"],
    },
  ];
  const elapsed = (task: Task) => {
    const run = scopedRuns
      .filter((item) => item.run.task_id === task.id)
      .sort((a, b) => b.run.updated_at.localeCompare(a.run.updated_at))[0];
    const end =
      run && terminalRun(run.run) ? Date.parse(run.run.updated_at) : now;
    const minutes = Math.max(
      0,
      Math.round((end - Date.parse(task.created_at)) / 60_000),
    );
    return minutes < 60
      ? `${minutes}m`
      : minutes < 2880
        ? `${Math.floor(minutes / 60)}h ${minutes % 60}m`
        : `${Math.floor(minutes / 1440)}d`;
  };
  const taskPane = (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="flex items-center justify-between gap-2 px-4 py-2.5">
        <span className="text-xs text-muted-foreground">
          <span className="font-mono tabular text-foreground">
            {progress.completed}/{progress.total}
          </span>{" "}
          {words.taskCounts}
        </span>
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={() => open({ kind: "task", workspace })}
        >
          <Plus />
          {copy.newTask}
        </Button>
      </div>
      {tasks.length === 0 && (
        <p className="px-4 py-6 text-center text-xs text-muted-foreground">
          {copy.noTasks}
        </p>
      )}
      {groups.map((group) => {
        const list = tasks.filter((task) =>
          group.statuses.includes(task.status),
        );
        return list.length === 0 ? null : (
          <section key={group.label} className="border-t border-border">
            <h3 className="flex items-center gap-2 px-4 pb-1 pt-3 text-[11px] font-medium text-faint">
              {group.label}
              <span className="font-mono tabular">{list.length}</span>
            </h3>
            <ul>
              {list.map((task) => (
                <li key={task.id}>
                  <button
                    type="button"
                    className="grid w-full grid-cols-[1fr_auto] gap-x-3 gap-y-0.5 px-4 py-2 text-left transition-colors hover:bg-accent"
                    onClick={() => open({ kind: "taskDetail", task })}
                  >
                    <span className="truncate text-[13px] text-foreground">
                      {task.title}
                    </span>
                    <span className="font-mono text-[11px] tabular text-faint">
                      {elapsed(task)}
                    </span>
                    <span className="flex min-w-0 items-center gap-2 text-[11px] text-muted-foreground">
                      <Phase status={task.status} className="text-[11px]" />
                      {ownerLabel(task) && (
                        <span className="truncate">{ownerLabel(task)}</span>
                      )}
                    </span>
                    {(task.dependencies ?? []).length > 0 && (
                      <span className="truncate text-right text-[11px] text-faint">
                        {task.dependencies
                          .map(
                            (id) =>
                              tasks.find((candidate) => candidate.id === id)
                                ?.title,
                          )
                          .filter(Boolean)
                          .join(", ")}
                      </span>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
  const resultPane = (
    <div className="grid min-h-0 flex-1 content-start gap-5 overflow-y-auto p-4">
      <ArtifactList artifacts={artifacts} />
      <ChannelFiles workspace={workspace.id} />
    </div>
  );
  const conversation = query.isError ? null : (
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
      system={system}
      requests={desktop ? undefined : decisions}
    />
  );
  const count = (value: number) => (
    <span className="font-mono text-[11px] tabular text-faint">{value}</span>
  );
  return (
    <section
      className="flex h-full min-h-0 min-w-0 flex-1 flex-col"
      aria-label={workspace.title}
    >
      <header className="flex shrink-0 flex-wrap items-center gap-x-6 gap-y-2 border-b border-border px-4 py-2.5 md:px-5 md:py-3">
        <div className="min-w-0 flex-1 basis-60">
          <div className="flex items-center gap-1.5 text-[11px]">
            <span className="truncate font-mono text-faint">
              {workspace.id}
            </span>
            <span aria-hidden className="text-faint">
              ·
            </span>
            <span className="inline-flex shrink-0 items-center gap-1.5 font-medium text-muted-foreground">
              <span
                aria-hidden
                className={cn(
                  "size-1.5 rounded-full",
                  state === "active"
                    ? "bg-brand-mark"
                    : state === "paused"
                      ? "bg-warning"
                      : state === "completed"
                        ? "bg-success"
                        : "bg-edge",
                )}
              />
              <span data-status={channelState[state]}>{words[state]}</span>
            </span>
          </div>
          <h1
            aria-label={`# ${workspace.title}`}
            className="truncate font-mono text-lg font-semibold tracking-tight text-foreground md:text-xl"
          >
            {workspace.title}
          </h1>
          <p className="line-clamp-1 text-xs text-muted-foreground md:text-[13px]">
            {workspace.goal}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            aria-label={copy.work}
            title={copy.work}
            onClick={() => showSheet("work")}
            className="mr-1 flex h-8 items-center gap-2 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <span>{view.tasks}</span>
            <span className="font-mono tabular text-foreground">
              {progress.completed}/{progress.total}
            </span>
            <SegmentedProgress
              tasks={tasks}
              label={view.tasks}
              className="w-20 md:w-28"
            />
          </button>
          <Button
            variant="ghost"
            size="sm"
            aria-label={copy.graphLink}
            onClick={() => graph()}
          >
            <Network />
            <span className="hidden lg:inline">{copy.graphLink}</span>
          </Button>
          <Button
            variant="ghost"
            size="sm"
            aria-label={view.files}
            onClick={() => tools("files")}
          >
            <FileText />
            <span className="hidden sm:inline">{view.files}</span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            aria-label={words.details}
            aria-expanded={statusOpen}
            onClick={() => showSheet("overview")}
          >
            <PanelRight />
            <span className="hidden sm:inline">{view.viewProgress}</span>
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={view.actions}>
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
              <DropdownMenuSeparator />
              <DropdownMenuItem
                disabled={control.disabled}
                onSelect={() => void control.toggle()}
              >
                {control.paused ? <Play /> : <Pause />}
                {control.label}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          {desktop && !dockOpen && (
            <Button
              variant="outline"
              size="sm"
              type="button"
              onClick={() => setDockOpen(true)}
            >
              <PanelRightOpen />
              {view.showDock}
            </Button>
          )}
        </div>
        {control.error && (
          <Alert className="basis-full">{control.error}</Alert>
        )}
      </header>
      <Tabs
        value={shownTab}
        onValueChange={(value) => setTab(value as typeof tab)}
        className="relative flex min-h-0 flex-1 flex-col md:flex-row"
      >
        {!desktop && (
          <TabsList className="bg-surface">
            <TabsTrigger value="conversation">{view.conversation}</TabsTrigger>
            <TabsTrigger value="canvas">{view.canvas}</TabsTrigger>
            <TabsTrigger value="tasks">
              {view.tasks}
              {count(tasks.length)}
            </TabsTrigger>
            <TabsTrigger value="results">
              {view.results}
              {count(artifacts.length)}
            </TabsTrigger>
          </TabsList>
        )}
        {desktop ? (
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            {unavailable}
            {canvas}
          </div>
        ) : (
          <TabsContent
            value="canvas"
            className="flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden"
          >
            {unavailable}
            {canvas}
          </TabsContent>
        )}
        <aside
          ref={setThreadContainer}
          aria-label={threadCopy[locale].thread}
          hidden={!thread || query.isError || shownTab !== "conversation"}
          className="absolute inset-0 z-40 flex min-h-0 flex-col bg-surface md:static md:z-auto md:w-[360px] md:shrink-0 md:border-l md:border-border"
        />
        <aside
          aria-label={view.details}
          hidden={desktop ? !dockOpen : tab === "canvas"}
          className="flex min-h-0 flex-1 flex-col bg-surface md:w-[400px] md:flex-none md:border-l md:border-border"
        >
          {desktop && (
            <div className="flex items-stretch border-b border-border">
              <TabsList className="flex-1 border-b-0">
                <TabsTrigger value="conversation">
                  {view.conversation}
                  {count(query.data?.messages?.length ?? 0)}
                </TabsTrigger>
                <TabsTrigger value="tasks">
                  {view.tasks}
                  {count(tasks.length)}
                </TabsTrigger>
                <TabsTrigger value="results">
                  {view.results}
                  {count(artifacts.length)}
                </TabsTrigger>
              </TabsList>
              <Button
                variant="ghost"
                size="icon"
                type="button"
                className="mr-1.5 self-center"
                aria-label={view.hideDock}
                onClick={() => setDockOpen(false)}
              >
                <PanelRightClose />
              </Button>
            </div>
          )}
          {!desktop && unavailable}
          <TabsContent
            value="conversation"
            forceMount
            className="flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden"
          >
            {conversation}
          </TabsContent>
          <TabsContent value="tasks" className="flex min-h-0 flex-1 flex-col">
            {taskPane}
          </TabsContent>
          <TabsContent value="results" className="flex min-h-0 flex-1 flex-col">
            {resultPane}
          </TabsContent>
        </aside>
      </Tabs>
      <Sheet open={statusOpen} onOpenChange={setStatusOpen}>
        <SheetContent closeLabel={t("close")} className="gap-0 p-0">
          <SheetHeader className="border-b border-border px-5 pb-3 pt-4">
            <SheetTitle>{view.progressTitle}</SheetTitle>
            <SheetDescription className="text-xs">
              {workspace.title}
            </SheetDescription>
          </SheetHeader>
          <Tabs
            value={sheetTab}
            onValueChange={(value) => setSheetTab(value as typeof sheetTab)}
            className="flex min-h-0 flex-1 flex-col"
          >
            <div className="flex items-stretch border-b border-border pr-2">
              <TabsList className="flex-1 border-b-0 px-5">
                <TabsTrigger value="overview">{view.overview}</TabsTrigger>
                <TabsTrigger value="work">{copy.work}</TabsTrigger>
                <TabsTrigger value="results">{copy.results}</TabsTrigger>
                <TabsTrigger value="activity">{copy.activity}</TabsTrigger>
              </TabsList>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                className="self-center"
                onClick={() => {
                  setStatusOpen(false);
                  tools("progress");
                }}
              >
                {view.recovery}
              </Button>
            </div>
            <TabsContent
              value="overview"
              className="flex min-h-0 flex-1 flex-col px-5 py-4"
            >
              <ChannelStatus
                workspace={workspace}
                data={data}
                tasks={tasks}
                runs={scopedRuns}
                waiting={scopedRequests.length}
                discovery={discovery}
                open={inspect}
                showTasks={() => setSheetTab("work")}
              />
            </TabsContent>
            <TabsContent
              value="work"
              className="grid min-h-0 flex-1 content-start gap-5 overflow-y-auto px-5 py-4"
            >
              <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                <dl
                  className="flex flex-1 flex-wrap gap-x-4 gap-y-1 text-xs"
                  aria-label={copy.taskProgress}
                >
                  {[
                    [copy.completed, `${progress.completed}/${progress.total}`],
                    [copy.active, progress.active],
                    [copy.attention, progress.attention],
                  ].map(([term, value]) => (
                    <div key={term} className="flex gap-1.5">
                      <dt className="text-faint">{term}</dt>
                      <dd className="font-mono tabular text-foreground">
                        {value}
                      </dd>
                    </div>
                  ))}
                </dl>
                <Button
                  variant="outline"
                  size="sm"
                  type="button"
                  onClick={() => inspect({ kind: "task", workspace })}
                >
                  <Plus />
                  {copy.newTask}
                </Button>
              </div>
              {tasks.length === 0 ? (
                <p className="text-xs text-muted-foreground">{copy.noTasks}</p>
              ) : (
                <ul className="divide-y divide-border rounded-md border border-border">
                  {tasks.map((task) => (
                    <li key={task.id}>
                      <button
                        type="button"
                        className="flex w-full items-start gap-3 px-3 py-2.5 text-left transition-colors hover:bg-accent"
                        onClick={() => inspect({ kind: "taskDetail", task })}
                      >
                        <span className="grid min-w-0 flex-1 gap-0.5">
                          <strong className="truncate text-[13px] font-medium text-foreground">
                            {task.title}
                          </strong>
                          <small className="line-clamp-2 text-xs text-muted-foreground">
                            {task.description}
                          </small>
                        </span>
                        <Badge value={task.status} />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <section className="grid gap-2">
                <h3
                  id={`participants-${workspace.id}`}
                  className="text-[11px] font-medium text-faint"
                >
                  {copy.participants}
                </h3>
                <ul
                  aria-labelledby={`participants-${workspace.id}`}
                  className="divide-y divide-border rounded-md border border-border"
                >
                  {agents.map((item) => {
                    const ref = {
                      id: item.run.agent_id,
                      version: item.run.agent_version,
                    };
                    const label = (
                      <>
                        <span className="truncate text-[13px] text-foreground">
                          {participantLabel(item)}
                        </span>
                        <small className="truncate font-mono text-[11px] text-faint">
                          <ReferenceName id={item.node} />
                        </small>
                      </>
                    );
                    return (
                      <li key={`${item.node}:${JSON.stringify(ref)}`}>
                        {item.node === data.node.id ? (
                          <button
                            className="flex w-full items-center justify-between gap-3 px-3 py-2 text-left transition-colors hover:bg-accent"
                            type="button"
                            onClick={() =>
                              graph(entityKey(item.node, "agent", ref))
                            }
                          >
                            {label}
                          </button>
                        ) : (
                          <div className="flex items-center justify-between gap-3 px-3 py-2">
                            {label}
                          </div>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </section>
            </TabsContent>
            <TabsContent
              value="results"
              className="grid min-h-0 flex-1 content-start gap-5 overflow-y-auto px-5 py-4"
            >
              <ArtifactList artifacts={artifacts} />
              <ChannelFiles workspace={workspace.id} />
            </TabsContent>
            <TabsContent
              value="activity"
              className="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto px-5 py-4"
            >
              <p className="text-xs text-muted-foreground">
                {copy.limitedHistory}
              </p>
              {scopedRuns.length > 0 && (
                <ul className="divide-y divide-border rounded-md border border-border">
                  {scopedRuns.map((item) => (
                    <li key={`${item.node}:${item.run.id}`}>
                      <button
                        type="button"
                        className="flex w-full items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-accent"
                        onClick={() => inspect({ kind: "run", ...item })}
                      >
                        <span className="grid min-w-0 flex-1">
                          <span className="truncate text-[13px] text-foreground">
                            {participantLabel(item)}
                          </span>
                          <small className="truncate font-mono text-[11px] text-faint">
                            <ReferenceName id={item.node} />
                          </small>
                        </span>
                        <Badge value={runDisplayStatus(item.run)} />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              {events.length > 0 && (
                <ul className="divide-y divide-border rounded-md border border-border">
                  {[...events].reverse().map((event) => (
                    <li key={event.id}>
                      <details className="group">
                        <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-xs transition-colors hover:bg-accent [&::-webkit-details-marker]:hidden">
                          <ChevronRight
                            aria-hidden
                            className="size-3.5 shrink-0 text-faint transition-transform group-open:rotate-90"
                          />
                          <span className="font-mono text-foreground">
                            {event.kind}
                          </span>
                          <span className="ml-auto font-mono tabular text-faint">
                            {new Date(event.created_at).toLocaleString(locale)}
                          </span>
                        </summary>
                        <div className="px-3 pb-3">
                          <RecordView value={event.data} />
                        </div>
                      </details>
                    </li>
                  ))}
                </ul>
              )}
            </TabsContent>
          </Tabs>
        </SheetContent>
      </Sheet>
    </section>
  );
}
