import React, { useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
  Link,
  useLocation,
} from "@tanstack/react-router";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  useReactTable,
  getCoreRowModel,
  flexRender,
  type ColumnDef,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Activity,
  ArrowUpRight,
  Boxes,
  Check,
  ChevronRight,
  CircleDot,
  Database,
  GitBranch,
  Globe,
  LayoutDashboard,
  ListTodo,
  MessageSquare,
  Network,
  Package as PackageIcon,
  Plus,
  Search as SearchIcon,
  ShieldCheck,
  Settings,
  Workflow,
  Zap,
} from "lucide-react";
import { subscribe } from "./api";
import { ApiError, AUTHENTICATION_EXPIRED } from "./transport";
import {
  state as getState,
  session as getSession,
  mesh as getMesh,
  discover,
  marketplace,
  runControl,
  remoteAction,
  messageCreate,
  taskAbandon,
  humanAnswer,
  packageInstall,
  workspaceGet,
  runGet,
  runMessage,
} from "./generated/aidash";
import type {
  Artifact,
  Discovery,
  Entry,
  HumanRequest,
  Invocation,
  MeshEvent,
  Package,
  Run,
  State,
  Task,
  Workspace,
} from "./types";
import {
  Badge,
  Empty,
  Field,
  JsonView,
  LocaleContext,
  Modal,
  Panel,
  useI18n,
  type Locale,
} from "./ui";
import {
  AssignForm,
  EntityForm,
  GoalForm,
  PeerForm,
  PublishForm,
  TaskForm,
  WorkspaceForm,
} from "./forms";
import { GenerationPage, GenerationAssignForm } from "./generation";
import { SemanticPage } from "./semantic";
import { AuthorizationPage } from "./authorization";
import { DeploymentPage } from "./deployment";
import { TransactionsPage } from "./transactions";
import "./style.css";
const sections = [
  ["overview", LayoutDashboard],
  ["agents", CircleDot],
  ["generation", Zap],
  ["authorization", ShieldCheck],
  ["transactions", GitBranch],
  ["semantic", Database],
  ["deployment", Boxes],
  ["clusters", Boxes],
  ["mesh", Network],
  ["tasks", ListTodo],
  ["workspaces", Workflow],
  ["conversations", MessageSquare],
  ["registry", GitBranch],
  ["marketplace", PackageIcon],
  ["events", Activity],
  ["settings", Settings],
] as const;
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, staleTime: 1000 } },
});
type DialogState = {
  kind: string;
  entity?: Entry;
  task?: Task;
  run?: Run;
  node?: string;
  workspace?: Workspace;
  request?: HumanRequest;
  artifact?: Artifact;
  package?: Package;
};

function App() {
  const [locale, setLocale] = useState<Locale>(
    (localStorage.getItem("aidash-locale") as Locale) || "ja-JP",
  );
  useEffect(() => {
    document.documentElement.lang = locale;
    localStorage.setItem("aidash-locale", locale);
  }, [locale]);
  return (
    <LocaleContext value={locale}>
      <Dashboard locale={locale} setLocale={setLocale} />
    </LocaleContext>
  );
}
function Dashboard({
  locale,
  setLocale,
}: {
  locale: Locale;
  setLocale: (l: Locale) => void;
}) {
  const { t, local } = useI18n();
  const client = useQueryClient();
  const location = useLocation();
  const section = location.pathname.split("/")[1] || "overview";
  const [token, setToken] = useState(
    sessionStorage.getItem("aidash-token") ?? "",
  );
  const [connected, setConnected] = useState(!!token);
  const [streamStatus, setStreamStatus] = useState<"live" | "reconnecting">(
    "reconnecting",
  );
  const [search, setSearch] = useState("");
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const session = useQuery({
    queryKey: ["session"],
    queryFn: () => getSession(),
    enabled: connected,
    refetchInterval: 5000,
  });
  const state = useQuery({
    queryKey: ["state"],
    queryFn: () => getState(),
    enabled: connected,
    refetchInterval: 5000,
  });
  useEffect(() => {
    const revoked = (event: Event) => {
      setError((event as CustomEvent<string>).detail);
      setConnected(false);
      setToken("");
      setDialog(null);
      sessionStorage.removeItem("aidash-token");
      client.clear();
    };
    window.addEventListener(AUTHENTICATION_EXPIRED, revoked);
    return () => window.removeEventListener(AUTHENTICATION_EXPIRED, revoked);
  }, [client]);
  const operator = session.data?.access.kind === "operator";
  const restrictedSection =
    !operator &&
    [
      "mesh",
      "marketplace",
      "transactions",
      "deployment",
      "authorization",
    ].includes(section);
  const showOrdinary =
    !state.isError &&
    !restrictedSection &&
    !["transactions", "deployment"].includes(section);
  const atomicPending =
    state.error instanceof ApiError && state.error.status === 503;
  const mesh = useQuery({
    queryKey: ["mesh"],
    queryFn: () => getMesh(),
    enabled: connected && operator,
    refetchInterval: 2000,
  });
  const discovery = useQuery({
    queryKey: ["discovery"],
    queryFn: () => discover({}),
    enabled: connected,
    refetchInterval: 10000,
  });
  const packages = useQuery({
    queryKey: ["packages"],
    queryFn: () => marketplace(),
    enabled: connected && operator,
  });
  useEffect(() => {
    if (!connected) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    void subscribe(
      controller.signal,
      () => {
        if (timer) return;
        timer = setTimeout(() => {
          void client.invalidateQueries({ queryKey: ["state"] });
          void client.invalidateQueries({ queryKey: ["packages"] });
          void client.invalidateQueries({ queryKey: ["generation"] });
          timer = undefined;
        }, 250);
      },
      setStreamStatus,
    );
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [connected, client]);
  const open = (d: DialogState) => {
    setError("");
    setDialog(d);
  };
  const submit = async (request: () => Promise<unknown>) => {
    if (busy) return false;
    setBusy(true);
    setError("");
    try {
      await request();
      await client.invalidateQueries();
      setDialog(null);
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  const control = async (run: Run, node: string, action: string) => {
    if (action === "cancel" && !confirm(t("confirmCancel"))) return;
    await submit(() =>
      node === state.data?.node.id
        ? runControl(run.id, { action })
        : remoteAction({ node_id: node, control: { run_id: run.id, action } }),
    );
  };
  if (!connected)
    return (
      <main className="login">
        <div className="login-brand">
          <span className="brand-symbol">a</span>
          <strong>Aidash</strong>
          <span>0.1</span>
        </div>
        <div className="login-card">
          <span className="eyebrow">AGENT MESH</span>
          <h1>{t("connect")}</h1>
          <p>{t("tokenHelp")}</p>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <form
            onSubmit={(e) => {
              e.preventDefault();
              sessionStorage.setItem("aidash-token", token);
              setError("");
              setConnected(true);
              void client.invalidateQueries();
            }}
          >
            <Field label={t("token")}>
              <input
                type="password"
                required
                value={token}
                autoComplete="off"
                onChange={(e) => setToken(e.target.value)}
              />
            </Field>
            <button className="primary">
              {t("connectAction")} <ArrowUpRight size={17} />
            </button>
          </form>
        </div>
      </main>
    );
  const data = state.data;
  const nodes = mesh.data?.nodes ?? [];
  const allRuns = data
    ? [
        ...data.runs.map((run) => ({ run, node: data.node.id })),
        ...nodes.flatMap((n) =>
          n.runs.map((run) => ({ run, node: n.node_id })),
        ),
      ]
    : [];
  const requests = data
    ? [
        ...data.human_requests.map((request) => ({
          request,
          node: data.node.id,
        })),
        ...nodes.flatMap((n) =>
          n.human_requests.map((request) => ({ request, node: n.node_id })),
        ),
      ].filter((x) => x.request.response === null)
    : [];
  const filtered = (entry: Entry) =>
    [
      entry.id,
      ...Object.values(entry.name),
      ...entry.capabilities,
      ...entry.languages,
    ]
      .join(" ")
      .toLowerCase()
      .includes(search.toLowerCase());
  const primary =
    section === "agents" || section === "registry" || section === "clusters"
      ? {
          label: "register",
          kind: section === "clusters" ? "cluster" : "entity",
        }
      : section === "workspaces"
        ? { label: "newWorkspace", kind: "workspace" }
        : section === "tasks"
          ? { label: "newTask", kind: "task" }
          : section === "marketplace"
            ? { label: "publish", kind: "publish" }
            : section === "settings" || section === "mesh"
              ? { label: "addPeer", kind: "peer" }
              : { label: "newGoal", kind: "goal" };
  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-symbol">a</span>
          <strong>Aidash</strong>
          <span className="version">0.1</span>
        </div>
        <div className="nav-label">{t("navGroup")}</div>
        <nav>
          {sections
            .filter(
              ([key]) =>
                operator ||
                ![
                  "mesh",
                  "marketplace",
                  "transactions",
                  "deployment",
                  "authorization",
                ].includes(key),
            )
            .map(([key, Icon]) => (
              <Link
                key={key}
                aria-label={t(key)}
                to="/$section"
                params={{ section: key }}
                className={section === key ? "nav-link selected" : "nav-link"}
              >
                <Icon size={18} />
                <span>{t(key)}</span>
                {key === "tasks" && data && (
                  <small>
                    {
                      data.tasks.filter((x) =>
                        ["RUNNING", "CLAIMED"].includes(x.status),
                      ).length
                    }
                  </small>
                )}
              </Link>
            ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="node-icon">
            <Network size={19} />
          </div>
          <div>
            <strong>
              {data?.node.id.replace("aidash://", "") ?? t("node")}
            </strong>
            <span>
              {data?.node.protocol_version ?? "0.1"} · {t("local")}
            </span>
          </div>
          <span className={`status-dot ${streamStatus}`} />
        </div>
      </aside>
      <div className="main-shell">
        <header className="topbar">
          <div className="breadcrumb">
            Aidash <ChevronRight size={14} /> <strong>{t(section)}</strong>
          </div>
          <div className="top-actions">
            <span className={`stream-status ${streamStatus}`}>
              <span className="status-dot" />
              {t(streamStatus)}
            </span>
            <select
              aria-label={t("language")}
              value={locale}
              onChange={(e) => setLocale(e.target.value as Locale)}
            >
              <option value="ja-JP">日本語</option>
              <option value="en-US">English</option>
            </select>
            <button
              className="avatar"
              onClick={() => open({ kind: "goal" })}
              aria-label={t("newGoal")}
            >
              <Plus size={18} />
            </button>
          </div>
        </header>
        <main className="content">
          <div className="page-heading">
            <div>
              <div className="eyebrow">
                {t("operations")} / {t(section).toUpperCase()}
              </div>
              <h1>
                {t(section)}
                <span className="heading-dot">.</span>
              </h1>
              <p>
                {section === "overview"
                  ? t("tagline")
                  : section === "mesh"
                    ? t("topologyHelp")
                    : section === "settings"
                      ? t("settingsHelp")
                      : section === "generation"
                        ? t("generationHelp")
                        : data?.node.id}
              </p>
            </div>
            {(operator ||
              ["workspace", "task", "goal"].includes(primary.kind)) &&
              !restrictedSection &&
              ![
                "generation",
                "authorization",
                "transactions",
                "semantic",
                "deployment",
              ].includes(section) && (
                <button
                  className="primary"
                  onClick={() => open({ kind: primary.kind })}
                >
                  <Plus size={17} />
                  {t(primary.label)}
                </button>
              )}
          </div>
          {error && !dialog && (
            <div className="error" role="alert">
              {error}
            </div>
          )}
          {state.isError && (
            <div className="error">
              <h3>
                {t(atomicPending ? "transactionWaiting" : "nodeUnavailable")}
              </h3>
              <p>
                {atomicPending
                  ? t("transactionWaitingHelp")
                  : state.error.message}
              </p>
              <button onClick={() => void state.refetch()}>{t("retry")}</button>
              <button
                onClick={() => {
                  setConnected(false);
                  sessionStorage.removeItem("aidash-token");
                  client.clear();
                }}
              >
                {t("disconnect")}
              </button>
            </div>
          )}
          {!data &&
            !state.isError &&
            !["transactions", "deployment"].includes(section) && (
              <div className="loading">{t("loading")}</div>
            )}
          {session.data && restrictedSection && (
            <div className="notice" role="status">
              {t("administratorsOnly")}
            </div>
          )}
          {section === "transactions" && operator && session.data && (
            <TransactionsPage nodeId={session.data.node_id} />
          )}
          {section === "deployment" && operator && session.data && (
            <DeploymentPage />
          )}
          {data && showOrdinary && (
            <>
              {(mesh.data?.errors.length ?? 0) > 0 && (
                <div className="notice">
                  {t("remoteUnavailable")}{" "}
                  <details>
                    <summary>{t("details")}</summary>
                    <JsonView value={mesh.data?.errors} />
                  </details>
                </div>
              )}
              {section === "authorization" && operator && (
                <AuthorizationPage entries={data.registry} />
              )}
              {section === "overview" && (
                <>
                  <div className="stats">
                    {[
                      [
                        t("activeAgents"),
                        data.registry.filter((e) => e.kind === "agent").length,
                        CircleDot,
                        Database,
                      ],
                      [
                        t("activeTasks"),
                        data.tasks.filter((t) =>
                          ["CLAIMED", "RUNNING"].includes(t.status),
                        ).length,
                        Zap,
                      ],
                      [
                        t("completedTasks"),
                        data.tasks.filter((t) => t.status === "COMPLETED")
                          .length,
                        Check,
                      ],
                      [t("connectedNodes"), 1 + nodes.length, Globe],
                    ].map(([label, count, Icon]) => {
                      const I = Icon as typeof Globe;
                      return (
                        <div className="stat" key={String(label)}>
                          <div>
                            <span>{String(label)}</span>
                            <I size={18} />
                          </div>
                          <strong>{String(count)}</strong>
                          <span className="stat-rule" />
                        </div>
                      );
                    })}
                  </div>
                  <div className="overview-grid">
                    <Panel
                      title={t("topology")}
                      action={
                        operator && (
                          <Link to="/$section" params={{ section: "mesh" }}>
                            {t("viewAll")} <ArrowUpRight size={14} />
                          </Link>
                        )
                      }
                    >
                      <MeshView
                        data={data}
                        discovery={discovery.data}
                        runs={allRuns}
                        compact
                      />
                    </Panel>
                    <Panel
                      title={t("needsAttention")}
                      action={<span className="count">{requests.length}</span>}
                    >
                      {requests.length === 0 ? (
                        <div className="quiet">
                          <Check size={25} />
                          <p>{t("noRequests")}</p>
                        </div>
                      ) : (
                        requests.slice(0, 4).map((x) => (
                          <button
                            className="request-card"
                            key={x.request.id}
                            onClick={() => open({ kind: "human", ...x })}
                          >
                            <Badge value={x.request.kind} />
                            <p>{x.request.prompt}</p>
                            <small>{x.node}</small>
                          </button>
                        ))
                      )}
                    </Panel>
                  </div>
                  <div className="overview-grid lower">
                    <Panel
                      title={t("tasks")}
                      action={
                        <Link to="/$section" params={{ section: "tasks" }}>
                          {t("viewAll")} <ArrowUpRight size={14} />
                        </Link>
                      }
                    >
                      <TaskTable
                        tasks={data.tasks.slice(-6)}
                        onSelect={(task) => open({ kind: "taskDetail", task })}
                      />
                    </Panel>
                    <Panel title={t("activity")}>
                      <EventList events={data.events.slice(-8).reverse()} />
                    </Panel>
                  </div>
                </>
              )}
              {["agents", "clusters", "registry"].includes(section) && (
                <>
                  <SearchBox value={search} setValue={setSearch} />
                  <div className="cards">
                    {data.registry
                      .filter(
                        (e) =>
                          (section === "registry" ||
                            e.kind ===
                              (section === "agents" ? "agent" : "cluster")) &&
                          filtered(e),
                      )
                      .map((e) => (
                        <button
                          className="entity-card"
                          key={`${e.id}@${e.version}`}
                          onClick={() =>
                            open({ kind: "entityDetail", entity: e })
                          }
                        >
                          <div className="card-top">
                            <span className={`entity-icon ${e.kind}`}>
                              <CircleDot size={22} />
                            </span>
                            <Badge
                              value={
                                allRuns.find(
                                  (r) =>
                                    r.run.agent_id === e.id &&
                                    ![
                                      "COMPLETED",
                                      "FAILED",
                                      "CANCELLED",
                                    ].includes(r.run.phase),
                                )?.run.phase ?? e.kind
                              }
                            />
                          </div>
                          <h3>{local(e.name)}</h3>
                          <p>{local(e.description)}</p>
                          <div className="tags">
                            {e.capabilities.slice(0, 3).map((c) => (
                              <span key={c}>{c}</span>
                            ))}
                          </div>
                          <footer>
                            <span>
                              {e.id} <b>v{e.version}</b>
                            </span>
                            <span>{e.languages.join(" / ")}</span>
                          </footer>
                        </button>
                      ))}
                  </div>
                  {data.registry.filter(
                    (e) =>
                      (section === "registry" ||
                        e.kind ===
                          (section === "agents" ? "agent" : "cluster")) &&
                      filtered(e),
                  ).length === 0 && <Empty />}
                </>
              )}
              {section === "mesh" && (
                <>
                  <Panel title={t("topology")}>
                    <MeshView
                      data={data}
                      discovery={discovery.data}
                      runs={allRuns}
                    />
                  </Panel>
                  <Panel title={t("execution")}>
                    <div className="run-list">
                      {allRuns.map((x) => (
                        <button
                          key={x.run.id}
                          onClick={() => open({ kind: "run", ...x })}
                        >
                          <span className="run-dot" />
                          <strong>{x.run.agent_id}</strong>
                          <span>{x.node}</span>
                          <Badge
                            value={
                              x.run.control === "PAUSED"
                                ? "PAUSED"
                                : x.run.phase
                            }
                          />
                          <small>
                            {t("step")} {x.run.step}
                          </small>
                        </button>
                      ))}
                    </div>
                    {allRuns.length === 0 && <Empty />}
                  </Panel>
                </>
              )}
              {section === "tasks" && (
                <>
                  <SearchBox value={search} setValue={setSearch} />
                  <Panel title={t("tasks")}>
                    <TaskTable
                      tasks={data.tasks.filter((t) =>
                        t.title.toLowerCase().includes(search.toLowerCase()),
                      )}
                      onSelect={(task) => open({ kind: "taskDetail", task })}
                    />
                  </Panel>
                </>
              )}
              {section === "workspaces" && (
                <>
                  <div className="cards workspace-cards">
                    {data.workspaces.map((w) => (
                      <button
                        className="workspace-card"
                        key={w.id}
                        onClick={() =>
                          open({ kind: "workspaceDetail", workspace: w })
                        }
                      >
                        <div className="card-top">
                          <Workflow size={23} />
                          <span className="count">
                            {
                              data.tasks.filter((t) => t.workspace_id === w.id)
                                .length
                            }
                          </span>
                        </div>
                        <h3>{w.title}</h3>
                        <p>{w.goal}</p>
                        <div className="progress-track">
                          <span
                            style={{
                              width: `${(data.tasks.filter((t) => t.workspace_id === w.id && t.status === "COMPLETED").length / Math.max(1, data.tasks.filter((t) => t.workspace_id === w.id).length)) * 100}%`,
                            }}
                          />
                        </div>
                        <footer>
                          {
                            data.tasks.filter(
                              (t) =>
                                t.workspace_id === w.id &&
                                t.status === "COMPLETED",
                            ).length
                          }{" "}
                          /{" "}
                          {
                            data.tasks.filter((t) => t.workspace_id === w.id)
                              .length
                          }{" "}
                          {t("COMPLETED")} <ArrowUpRight size={16} />
                        </footer>
                      </button>
                    ))}
                  </div>
                  {data.workspaces.length === 0 && <Empty />}
                  {data.workspaces.map((w) => (
                    <Panel key={w.id} title={w.title}>
                      <TaskBoard
                        tasks={data.tasks.filter(
                          (t) => t.workspace_id === w.id,
                        )}
                        select={(task) => open({ kind: "taskDetail", task })}
                      />
                    </Panel>
                  ))}
                </>
              )}
              {section === "conversations" && (
                <>
                  {data.conversations.map((c) => (
                    <Panel
                      key={c.id}
                      title={
                        data.workspaces.find((w) => w.id === c.workspace_id)
                          ?.title ?? c.id
                      }
                      action={<span className="muted">{c.target}</span>}
                    >
                      <ConversationView
                        workspace={c.workspace_id}
                        send={(body) =>
                          submit(() => messageCreate(c.workspace_id, body))
                        }
                      />
                    </Panel>
                  ))}
                  {data.conversations.length === 0 && <Empty />}
                </>
              )}
              {section === "marketplace" && (
                <>
                  <SearchBox value={search} setValue={setSearch} />
                  <div className="cards">
                    {packages.data
                      ?.filter((p) => filtered(p.manifest.entity))
                      .map((p) => (
                        <button
                          key={`${p.id}@${p.version}`}
                          className="entity-card"
                          onClick={() => open({ kind: "package", package: p })}
                        >
                          <div className="card-top">
                            <PackageIcon size={26} />
                            <Badge value={p.manifest.entity.kind} />
                          </div>
                          <h3>{local(p.manifest.entity.name)}</h3>
                          <p>{local(p.manifest.entity.description)}</p>
                          <div className="tags">
                            {p.manifest.entity.capabilities.map((c) => (
                              <span key={c}>{c}</span>
                            ))}
                          </div>
                          <footer>
                            <span>
                              {p.manifest.author} · v{p.version}
                            </span>
                            <span>
                              {data.installations.some(
                                (i) => i.id === p.id && i.version === p.version,
                              )
                                ? t("installed")
                                : t("details")}
                            </span>
                          </footer>
                        </button>
                      ))}
                  </div>
                  {packages.data?.length === 0 && <Empty />}
                </>
              )}
              {section === "events" && (
                <Panel
                  title={t("stream")}
                  action={
                    <span className="stream-status">
                      <span className="status-dot" />
                      {t(streamStatus)}
                    </span>
                  }
                >
                  <EventList events={[...data.events].reverse()} tall />
                </Panel>
              )}
              {section === "generation" && <GenerationPage data={data} />}
              {section === "semantic" && <SemanticPage data={data} />}
              {section === "settings" && (
                <>
                  <Panel title={t("node")}>
                    <dl>
                      <dt>{t("entityId")}</dt>
                      <dd>{data.node.id}</dd>
                      <dt>{t("endpoint")}</dt>
                      <dd>{data.node.endpoint}</dd>
                      <dt>{t("protocol")}</dt>
                      <dd>{data.node.protocol_version}</dd>
                    </dl>
                  </Panel>
                  <Panel title={t("signedInAs")}>
                    {data.access.kind === "subject" ? (
                      <dl>
                        <dt>{t("tenant")}</dt>
                        <dd>{data.access.tenant}</dd>
                        <dt>{t("subject")}</dt>
                        <dd>{data.access.subject}</dd>
                      </dl>
                    ) : (
                      <p>{t("administrator")}</p>
                    )}
                  </Panel>
                  {operator && (
                    <Panel
                      title={t("connectedNodes")}
                      action={
                        <button onClick={() => open({ kind: "peer" })}>
                          <Plus size={16} />
                          {t("addPeer")}
                        </button>
                      }
                    >
                      {data.peers.map((p) => (
                        <div className="peer-row" key={p.node_id}>
                          <Globe size={20} />
                          <div>
                            <strong>{p.node_id}</strong>
                            <p>{p.endpoint}</p>
                          </div>
                          <Badge value={p.enabled ? "ACTIVE" : "PAUSED"} />
                        </div>
                      ))}
                      {data.peers.length === 0 && <Empty />}
                    </Panel>
                  )}
                  <button
                    onClick={() => {
                      setConnected(false);
                      sessionStorage.removeItem("aidash-token");
                      client.clear();
                    }}
                  >
                    {t("disconnect")}
                  </button>
                </>
              )}
            </>
          )}
        </main>
        <footer className="page-footer">
          <span>Aidash · v0.1.0</span>
          <span>{t("federatedMesh")}</span>
        </footer>
      </div>
      {dialog && data && (
        <Modal
          title={t(
            (
              {
                goal: "newGoal",
                workspace: "newWorkspace",
                task: "newTask",
                entity: "register",
                cluster: "register",
                peer: "addPeer",
                publish: "publish",
                taskDetail: "task",
                entityDetail: "agent",
                run: "execution",
                human: "human",
                workspaceDetail: "workspace",
                package: "package",
                assign: "delegate",
                generate: "generationAssign",
              } as Record<string, string>
            )[dialog.kind] ?? dialog.kind,
          )}
          close={() => setDialog(null)}
        >
          {error && (
            <div className="error" role="alert">
              {error}
            </div>
          )}
          <div className={busy ? "form-busy" : ""}>
            {dialog.kind === "goal" && <GoalForm data={data} submit={submit} />}{" "}
            {dialog.kind === "workspace" && <WorkspaceForm submit={submit} />}{" "}
            {dialog.kind === "task" && <TaskForm data={data} submit={submit} />}{" "}
            {["entity", "cluster"].includes(dialog.kind) && (
              <EntityForm
                data={data}
                submit={submit}
                initial={
                  dialog.kind === "cluster"
                    ? "cluster"
                    : section === "registry"
                      ? "model"
                      : "agent"
                }
              />
            )}{" "}
            {dialog.kind === "peer" && <PeerForm submit={submit} />}{" "}
            {dialog.kind === "publish" && (
              <PublishForm data={data} submit={submit} />
            )}
            {dialog.kind === "generate" &&
              dialog.task &&
              data.access.kind === "subject" && (
                <GenerationAssignForm
                  tenant={data.access.tenant}
                  task={dialog.task}
                  submit={submit}
                />
              )}
            {dialog.kind === "assign" && dialog.task && (
              <AssignForm
                data={data}
                task={dialog.task}
                discovery={discovery.data ?? { agents: [], errors: [] }}
                submit={submit}
              />
            )}
            {dialog.kind === "taskDetail" &&
              dialog.task &&
              (() => {
                const task =
                  data.tasks.find((t) => t.id === dialog.task?.id) ??
                  dialog.task;
                return (
                  <>
                    <h3>{task.title}</h3>
                    <p>{task.description}</p>
                    <Badge value={task.status} />
                    <dl>
                      <dt>{t("owner")}</dt>
                      <dd>{task.owner ?? t("noAssignment")}</dd>
                      <dt>{t("revision")}</dt>
                      <dd>{task.revision}</dd>
                    </dl>
                    <JsonView value={task.requirements} />
                    {task.status === "OPEN" &&
                      data.access.kind === "subject" && (
                        <button
                          onClick={() => open({ kind: "generate", task })}
                        >
                          {t("generationAssign")}
                        </button>
                      )}
                    {task.status === "OPEN" && (
                      <button
                        className="primary"
                        onClick={() => open({ kind: "assign", task })}
                      >
                        {t("delegate")}
                      </button>
                    )}
                    {["FAILED", "BLOCKED", "CANCELLED"].includes(
                      task.status,
                    ) && (
                      <form
                        onSubmit={(event) => {
                          event.preventDefault();
                          const reason = String(
                            new FormData(event.currentTarget).get("reason"),
                          );
                          void submit(() =>
                            taskAbandon(task.id, {
                              revision: task.revision,
                              reason,
                            }),
                          );
                        }}
                      >
                        <Field label={t("abandonReason")}>
                          <textarea name="reason" required rows={2} />
                        </Field>
                        <button className="danger">{t("abandonTask")}</button>
                        <p className="muted">{t("abandonHelp")}</p>
                      </form>
                    )}
                    {allRuns
                      .filter((x) => x.run.task_id === task.id)
                      .map((x) => (
                        <button
                          key={x.run.id}
                          onClick={() => open({ kind: "run", ...x })}
                        >
                          {t("execution")} <ArrowUpRight size={16} />
                        </button>
                      ))}
                    <ArtifactList
                      artifacts={data.artifacts.filter(
                        (a) => a.task_id === task.id,
                      )}
                    />
                  </>
                );
              })()}
            {dialog.kind === "entityDetail" && dialog.entity && (
              <>
                <h3>{local(dialog.entity.name)}</h3>
                <p>{local(dialog.entity.description)}</p>
                <div className="tags">
                  {dialog.entity.capabilities.map((c) => (
                    <span key={c}>{c}</span>
                  ))}
                </div>
                <h4>{t("execution")}</h4>
                {allRuns
                  .filter(
                    (x) =>
                      x.run.agent_id === dialog.entity?.id &&
                      x.run.agent_version === dialog.entity?.version,
                  )
                  .map((x) => (
                    <button
                      className="run-choice"
                      key={x.run.id}
                      onClick={() => open({ kind: "run", ...x })}
                    >
                      <Badge value={x.run.phase} />
                      {x.run.task_id.slice(0, 8)}
                      <ArrowUpRight size={15} />
                    </button>
                  ))}
                <h4>{t("metadata")}</h4>
                <JsonView value={dialog.entity} />
              </>
            )}
            {dialog.kind === "run" && dialog.run && (
              <RunDetails
                run={
                  allRuns.find((x) => x.run.id === dialog.run?.id)?.run ??
                  dialog.run
                }
                node={dialog.node ?? data.node.id}
                localNode={data.node.id}
                agent={
                  (
                    discovery.data?.agents ??
                    data.registry
                      .filter((e) => e.kind === "agent")
                      .map((entity) => ({ node_id: data.node.id, entity }))
                  ).find(
                    (a) =>
                      a.node_id === (dialog.node ?? data.node.id) &&
                      a.entity.id === dialog.run?.agent_id &&
                      a.entity.version === dialog.run?.agent_version,
                  )?.entity
                }
                events={data.events}
                remoteInvocations={nodes.flatMap((n) => n.invocations)}
                control={control}
              />
            )}
            {dialog.kind === "human" && dialog.request && (
              <HumanForm
                request={dialog.request}
                answer={(response) =>
                  submit(() =>
                    dialog.node === data.node.id
                      ? humanAnswer(dialog.request!.id, response)
                      : remoteAction({
                          node_id: dialog.node ?? data.node.id,
                          control: {
                            run_id: dialog.request!.run_id,
                            action: "answer",
                            request_id: dialog.request!.id,
                            response,
                          },
                        }),
                  )
                }
              />
            )}
            {dialog.kind === "workspaceDetail" && dialog.workspace && (
              <>
                <h3>{dialog.workspace.title}</h3>
                <p>{dialog.workspace.goal}</p>
                <TaskBoard
                  tasks={data.tasks.filter(
                    (t) => t.workspace_id === dialog.workspace?.id,
                  )}
                  select={(task) => open({ kind: "taskDetail", task })}
                />
                <ArtifactList
                  artifacts={data.artifacts.filter(
                    (a) => a.workspace_id === dialog.workspace?.id,
                  )}
                />
                <ConversationView
                  workspace={dialog.workspace.id}
                  send={(body) =>
                    submit(() => messageCreate(dialog.workspace!.id, body))
                  }
                />
              </>
            )}
            {dialog.kind === "package" && dialog.package && (
              <>
                <h3>{local(dialog.package.manifest.entity.name)}</h3>
                <p>{local(dialog.package.manifest.entity.description)}</p>
                <dl>
                  {[
                    ["author", dialog.package.manifest.author],
                    ["version", dialog.package.version],
                    [
                      "languages",
                      dialog.package.manifest.entity.languages.join(", "),
                    ],
                    [
                      "capabilities",
                      dialog.package.manifest.entity.capabilities.join(", "),
                    ],
                    [
                      "permissions",
                      dialog.package.manifest.permissions.join(", "),
                    ],
                    [
                      "dependencies",
                      (dialog.package.manifest.dependencies ?? [])
                        .map((d) => `${d.id}@${d.version}`)
                        .join(", "),
                    ],
                  ].map(([k, v]) => (
                    <React.Fragment key={k}>
                      <dt>{t(k)}</dt>
                      <dd>{v || "—"}</dd>
                    </React.Fragment>
                  ))}
                </dl>
                <code className="digest">{dialog.package.digest}</code>
                <button
                  className="primary"
                  onClick={() =>
                    void submit(() =>
                      packageInstall(
                        dialog.package!.id,
                        dialog.package!.version,
                        { digest: dialog.package!.digest, config: {} },
                      ),
                    )
                  }
                >
                  {t("install")}
                </button>
              </>
            )}
          </div>
        </Modal>
      )}
    </div>
  );
}

function SearchBox({
  value,
  setValue,
}: {
  value: string;
  setValue: (v: string) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="search">
      <SearchIcon size={18} />
      <input
        aria-label={t("search")}
        placeholder={t("filter")}
        value={value}
        onChange={(e) => setValue(e.target.value)}
      />
    </div>
  );
}
function TaskTable({
  tasks,
  onSelect,
}: {
  tasks: Task[];
  onSelect: (t: Task) => void;
}) {
  const { t } = useI18n();
  const columns = useMemo<ColumnDef<Task>[]>(
    () => [
      {
        accessorKey: "title",
        header: t("task"),
        cell: (c) => (
          <button
            className="text-button"
            onClick={() => onSelect(c.row.original)}
          >
            {String(c.getValue())}
          </button>
        ),
      },
      {
        accessorKey: "status",
        header: t("status"),
        cell: (c) => <Badge value={String(c.getValue())} />,
      },
      {
        accessorKey: "owner",
        header: t("owner"),
        cell: (c) => (
          <span className="mono muted">
            {String(c.getValue() ?? t("noAssignment")).replace("aidash://", "")}
          </span>
        ),
      },
    ],
    [t, onSelect],
  );
  const table = useReactTable({
    data: tasks,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });
  return tasks.length === 0 ? (
    <Empty />
  ) : (
    <div className="table-wrap">
      <table>
        <thead>
          {table.getHeaderGroups().map((g) => (
            <tr key={g.id}>
              {g.headers.map((h) => (
                <th key={h.id}>
                  {flexRender(h.column.columnDef.header, h.getContext())}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((r) => (
            <tr key={r.id}>
              {r.getVisibleCells().map((c) => (
                <td key={c.id}>
                  {flexRender(c.column.columnDef.cell, c.getContext())}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
function TaskBoard({
  tasks,
  select,
}: {
  tasks: Task[];
  select: (t: Task) => void;
}) {
  const { t } = useI18n();
  const statuses = [
    "OPEN",
    "CLAIMED",
    "RUNNING",
    "COMPLETED",
    ...["FAILED", "BLOCKED", "CANCELLED", "ABANDONED"].filter((s) =>
      tasks.some((t) => t.status === s),
    ),
  ];
  return (
    <div className="board">
      {statuses.map((status) => (
        <div className="board-column" key={status}>
          <div className="board-heading">
            <Badge value={status} />
            <span>{tasks.filter((t) => t.status === status).length}</span>
          </div>
          {tasks
            .filter((t) => t.status === status)
            .map((task) => (
              <button
                key={task.id}
                onClick={() => select(task)}
                className="task-card"
              >
                <strong>{task.title}</strong>
                <p>{task.description}</p>
                <footer>
                  <CircleDot size={13} />
                  {task.owner?.split("/agents/")[1] ?? t("noAssignment")}
                </footer>
              </button>
            ))}
        </div>
      ))}
    </div>
  );
}
function MeshView({
  data,
  discovery,
  compact = false,
  runs,
}: {
  data: State;
  runs: { run: Run; node: string }[];
  discovery?: Discovery;
  compact?: boolean;
}) {
  const { t, local } = useI18n();
  const agents =
    discovery?.agents ??
    data.registry
      .filter((e) => e.kind === "agent")
      .map((entity) => ({ entity, node_id: data.node.id }));
  const nodeIds = [
    data.node.id,
    ...data.peers.filter((p) => p.enabled).map((p) => p.node_id),
  ];
  const width = Math.max(600, nodeIds.length * 300);
  const height = compact
    ? 260
    : Math.max(
        310,
        ...nodeIds.map(
          (n) => 150 + agents.filter((a) => a.node_id === n).length * 64,
        ),
      );
  const shown = compact
    ? agents.filter(
        (a, i, all) =>
          all.filter((b) => b.node_id === a.node_id).indexOf(a) < 2,
      )
    : agents;
  const positions = new Map(
    shown.map((a) => {
      const i = nodeIds.indexOf(a.node_id);
      const j = shown.filter((b) => b.node_id === a.node_id).indexOf(a);
      return [
        `${a.node_id}/agents/${a.entity.id}@${a.entity.version}`,
        { x: i * 300 + 78, y: 127 + j * 64 },
      ];
    }),
  );
  const communications = data.tasks.filter(
    (task) =>
      task.owner &&
      task.created_by !== task.owner &&
      positions.has(task.created_by) &&
      positions.has(task.owner),
  );
  const arrowId = compact ? "communication-small" : "communication";
  return (
    <div className={`mesh-canvas ${compact ? "compact" : ""}`}>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={t("topology")}
      >
        <defs>
          <pattern
            id={compact ? "dots-small" : "dots"}
            width="18"
            height="18"
            patternUnits="userSpaceOnUse"
          >
            <circle cx="1" cy="1" r="1" fill="#dae2dd" />
          </pattern>
          <marker
            id={arrowId}
            markerWidth="6"
            markerHeight="6"
            refX="5"
            refY="3"
            orient="auto"
          >
            <path d="M0 0 L6 3 L0 6" fill="#7ba898" />
          </marker>
        </defs>
        <rect
          width={width}
          height={height}
          fill={`url(#${compact ? "dots-small" : "dots"})`}
        />
        {nodeIds.length > 1 && (
          <path
            d={`M150 58 H${(nodeIds.length - 1) * 300 + 150}`}
            stroke="#85a89b"
            strokeWidth="2"
            strokeDasharray="5 5"
          />
        )}
        {communications.map((task) => {
          const from = positions.get(task.created_by)!;
          const to = positions.get(task.owner!)!;
          const sameNode = from.x === to.x;
          const path = sameNode
            ? `M${from.x} ${from.y} C${from.x - 48} ${from.y},${to.x - 48} ${to.y},${to.x - 4} ${to.y}`
            : `M${from.x + 190} ${from.y} C${from.x + 235} ${from.y},${to.x - 42} ${to.y},${to.x - 4} ${to.y}`;
          return (
            <path
              key={task.id}
              d={path}
              fill="none"
              stroke="#468a70"
              strokeWidth="1.8"
              markerEnd={`url(#${arrowId})`}
            >
              <title>{`${task.title}: ${task.created_by} → ${task.owner} (${t(task.status)})`}</title>
            </path>
          );
        })}
        {nodeIds.map((node, i) => {
          const nodeAgents = shown.filter((a) => a.node_id === node);
          return (
            <g key={node}>
              <rect
                x={i * 300 + 25}
                y={25}
                width={250}
                height={65}
                rx={12}
                fill={i === 0 ? "#184e41" : "#ecf1ed"}
                stroke="#bfd0c5"
              />
              <circle
                cx={i * 300 + 48}
                cy={56}
                r={5}
                fill={i === 0 ? "#bce6bd" : "#619278"}
              />
              <text
                x={i * 300 + 64}
                y={54}
                fill={i === 0 ? "#fff" : "#285240"}
                fontSize="13"
                fontWeight="600"
              >
                {node.replace("aidash://", "")}
              </text>
              <text
                x={i * 300 + 64}
                y={74}
                fill={i === 0 ? "#b6d1c2" : "#76867d"}
                fontSize="10"
              >
                {nodeAgents.length} {t("agents")}
              </text>
              {nodeAgents.map((a, j) => {
                const cluster = (
                  a.entity.config.cluster as { id?: string } | null
                )?.id;
                const run = runs.find(
                  ({ run: r, node }) =>
                    node === a.node_id &&
                    r.agent_id === a.entity.id &&
                    r.agent_version === a.entity.version &&
                    !["COMPLETED", "FAILED", "CANCELLED"].includes(r.phase),
                );
                return (
                  <g key={`${a.entity.id}@${a.entity.version}`}>
                    <path
                      d={`M${i * 300 + 55} ${j === 0 ? 90 : 125 + (j - 1) * 64} V${125 + j * 64} H${i * 300 + 78}`}
                      fill="none"
                      stroke="#a7bdb1"
                      strokeWidth="1.5"
                    />
                    <rect
                      x={i * 300 + 78}
                      y={104 + j * 64}
                      width={190}
                      height={46}
                      rx={8}
                      fill="white"
                      stroke="#d8e2db"
                    />
                    <circle
                      cx={i * 300 + 93}
                      cy={127 + j * 64}
                      r={4}
                      fill={run ? "#46a374" : "#b9c5bd"}
                    />
                    <text
                      x={i * 300 + 106}
                      y={123 + j * 64}
                      fill="#244438"
                      fontSize="11"
                      fontWeight="600"
                    >
                      {local(a.entity.name).slice(0, 22)}
                    </text>
                    <text
                      x={i * 300 + 106}
                      y={138 + j * 64}
                      fill="#74867a"
                      fontSize="9"
                    >
                      {cluster ?? a.entity.languages.join(" / ")}
                    </text>
                  </g>
                );
              })}
            </g>
          );
        })}
      </svg>
      <div className="mesh-legend">
        <span>
          {t("taskCommunication")} · {communications.length}
        </span>
        <span>
          <i className="legend-node" />
          {t("node")}
        </span>
        <span>
          <i className="legend-agent" />
          {t("agent")}
        </span>
        <span className="muted">
          {
            data.tasks.filter(
              (t) => t.owner && !t.owner.startsWith(data.node.id + "/"),
            ).length
          }{" "}
          · {t("remoteTasks")}
        </span>
      </div>
    </div>
  );
}
function EventList({
  events,
  tall = false,
}: {
  events: MeshEvent[];
  tall?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const { locale } = useI18n();
  const virtual = useVirtualizer({
    count: events.length,
    getScrollElement: () => ref.current,
    estimateSize: () => 76,
    overscan: 5,
  });
  if (events.length === 0) return <Empty />;
  return (
    <div
      ref={ref}
      className="event-scroll"
      style={{ height: tall ? 600 : 330 }}
    >
      <div style={{ height: virtual.getTotalSize(), position: "relative" }}>
        {virtual.getVirtualItems().map((item) => {
          const event = events[item.index];
          return (
            <div
              key={event.id}
              ref={virtual.measureElement}
              data-index={item.index}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${item.start}px)`,
              }}
            >
              <details className="event-item">
                <summary>
                  <span
                    className={`event-dot ${event.kind.includes("failed") ? "failed" : ""}`}
                  />
                  <div>
                    <strong>{event.kind}</strong>
                    <small>
                      {event.node_id.replace("aidash://", "")} · #
                      {event.sequence}
                    </small>
                  </div>
                  <time>
                    {new Date(event.created_at).toLocaleTimeString(locale, {
                      hour: "2-digit",
                      minute: "2-digit",
                      second: "2-digit",
                    })}
                  </time>
                </summary>
                <JsonView value={event.data} />
              </details>
            </div>
          );
        })}
      </div>
    </div>
  );
}
function ArtifactList({ artifacts }: { artifacts: Artifact[] }) {
  const { t } = useI18n();
  return (
    <div className="artifact-list">
      <h4>{t("artifacts")}</h4>
      {artifacts.length === 0 ? (
        <p className="muted">—</p>
      ) : (
        artifacts.map((a) => (
          <details key={a.id}>
            <summary>
              <PackageIcon size={16} />
              <strong>{a.name}</strong>
              <Badge value={a.kind} />
            </summary>
            <JsonView value={a.content} />
            <small className="muted">{a.created_by}</small>
          </details>
        ))
      )}
    </div>
  );
}
function ConversationView({
  workspace,
  send,
}: {
  workspace: string;
  send: (body: { content: string }) => Promise<boolean>;
}) {
  const { t } = useI18n();
  const query = useQuery({
    queryKey: ["workspace", workspace],
    queryFn: () => workspaceGet(workspace),
    refetchInterval: 2000,
  });
  return (
    <div className="conversation">
      <div className="messages">
        {query.data?.messages.map((m) => (
          <div
            className={`message ${m.sender === "human" ? "from-human" : ""}`}
            key={m.id}
          >
            <span>
              {m.sender === "human"
                ? t("human")
                : (m.sender.split("/agents/")[1] ?? m.sender)}
            </span>
            <p>{m.content}</p>
          </div>
        ))}
      </div>
      <form
        className="message-form"
        onSubmit={(e) => {
          e.preventDefault();
          const form = e.currentTarget;
          const d = new FormData(form);
          void send({ content: String(d.get("content")) }).then((sent) => {
            if (sent) form.reset();
          });
        }}
      >
        <input
          name="content"
          aria-label={t("message")}
          placeholder={t("message")}
          required
        />
        <button className="primary">{t("send")}</button>
      </form>
      {query.data && <ArtifactList artifacts={query.data.artifacts} />}
    </div>
  );
}
function RunDetails({
  run,
  node,
  localNode,
  remoteInvocations,
  agent,
  events,
  control,
}: {
  run: Run;
  node: string;
  localNode: string;
  remoteInvocations: Invocation[];
  agent?: Entry;
  events: MeshEvent[];
  control: (r: Run, n: string, a: string) => Promise<void>;
}) {
  const { t } = useI18n();
  const [messageError, setMessageError] = useState("");
  const [messageSent, setMessageSent] = useState(false);
  const model = agent?.config.model as
    | { id?: string; version?: string }
    | undefined;
  const details = useQuery({
    queryKey: ["run", run.id],
    queryFn: () => runGet(run.id),
    enabled: node === localNode,
    refetchInterval: 2000,
  });
  const calls =
    node === localNode
      ? (details.data?.invocations ?? [])
      : remoteInvocations.filter((i) => i.run_id === run.id);
  return (
    <>
      <h3>{run.agent_id}</h3>
      <div className="detail-status">
        <Badge value={run.phase} />
        <Badge value={run.control} />
        <span className="muted">{node}</span>
      </div>
      <dl>
        <dt>{t("task")}</dt>
        <dd>{run.task_id}</dd>
        <dt>{t("model")}</dt>
        <dd>{model ? `${model.id}@${model.version}` : "—"}</dd>
        <dt>{t("step")}</dt>
        <dd>{run.step}</dd>
        <dt>{t("context")}</dt>
        <dd>
          {run.context.usage?.input_tokens ?? 0} /{" "}
          {run.context.usage?.context_window ?? "—"}
        </dd>
      </dl>
      <div className="button-row">
        {!["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase) && (
          <>
            <button
              onClick={() =>
                void control(
                  run,
                  node,
                  run.control === "PAUSED" ? "resume" : "pause",
                )
              }
            >
              {t(run.control === "PAUSED" ? "resume" : "pause")}
            </button>
            <button
              className="danger"
              onClick={() => void control(run, node, "cancel")}
            >
              {t("cancel")}
            </button>
          </>
        )}
      </div>
      {run.error && <div className="error">{run.error}</div>}
      {!["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase) && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            const form = e.currentTarget;
            const content = String(new FormData(form).get("content"));
            if (form.dataset.messageContent !== content) {
              form.dataset.messageKey = crypto.randomUUID();
              form.dataset.messageContent = content;
            }
            const idempotency_key =
              form.dataset.messageKey ?? crypto.randomUUID();
            form.dataset.messageKey = idempotency_key;
            setMessageError("");
            setMessageSent(false);
            try {
              await (node === localNode
                ? runMessage(run.id, { content, idempotency_key })
                : remoteAction({
                    node_id: node,
                    control: {
                      run_id: run.id,
                      action: "message",
                      content,
                      idempotency_key,
                    },
                  }));
              form.reset();
              delete form.dataset.messageKey;
              delete form.dataset.messageContent;
              setMessageSent(true);
            } catch (error) {
              setMessageError(String(error));
            }
          }}
        >
          <Field label={t("message")}>
            <textarea name="content" required rows={3} />
          </Field>
          <button>{t("send")}</button>
          {messageError && (
            <p role="alert" className="error">
              {messageError}
            </p>
          )}
          {messageSent && <p role="status">{t("messageSent")}</p>}
        </form>
      )}
      <h4>{t("toolCalls")}</h4>
      {calls.map((c) => (
        <details className="call-detail" key={c.idempotency_key}>
          <summary>
            <Badge value={c.status} />
            <strong>{c.tool}</strong>
          </summary>
          <h5>{t("input")}</h5>
          <JsonView value={c.input} />
          <h5>{t("result")}</h5>
          <JsonView value={c.result} />
        </details>
      ))}
      <h4>{t("recentEvents")}</h4>
      <EventList
        events={events.filter((e) => {
          const data = JSON.stringify(e.data);
          return data.includes(run.id) || data.includes(run.task_id);
        })}
      />
      <details>
        <summary>{t("context")}</summary>
        <JsonView value={run.context} />
      </details>
      {details.data && (
        <details>
          <summary>{t("memory")}</summary>
          <JsonView value={details.data.memory} />
        </details>
      )}
    </>
  );
}
function HumanForm({
  request,
  answer,
}: {
  request: HumanRequest;
  answer: (response: unknown) => Promise<unknown>;
}) {
  const { t } = useI18n();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        const raw = String(d.get("response"));
        let response: unknown;
        try {
          response = JSON.parse(raw);
        } catch {
          response = raw;
        }
        void answer(response);
      }}
    >
      <Badge value={request.kind} />
      <p className="human-prompt">{request.prompt}</p>
      <Field label={t("answer")}>
        <textarea
          name="response"
          required
          rows={5}
          placeholder={t("answerHint")}
        />
      </Field>
      <button className="primary">{t("send")}</button>
    </form>
  );
}
const rootRoute = createRootRoute({ component: App });
const indexRoute = createRoute({ getParentRoute: () => rootRoute, path: "/" });
const sectionRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/$section",
});
const router = createRouter({
  routeTree: rootRoute.addChildren([indexRoute, sectionRoute]),
});
declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </React.StrictMode>,
);
