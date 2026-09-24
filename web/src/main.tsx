import { lazy, Suspense, useEffect, useRef, useState } from "react";
import React from "react";
import { createRoot } from "react-dom/client";
import { DisplayProvider, ReferenceName } from "./record-view";
import {
  createRootRoute,
  createRoute,
  createRouter,
  Link,
  RouterProvider,
  useLocation,
  useNavigate,
} from "@tanstack/react-router";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { MessageSquare, Network, Settings, Plus } from "lucide-react";
import {
  state as getState,
  session as getSession,
  mesh as getMesh,
  discover,
  marketplace,
} from "./generated/aidash";
import { subscribe } from "./event-stream";
import {
  AUTHENTICATION_EXPIRED,
  csrfToken,
  dashboardContext,
  selectDashboardContext,
} from "./transport";
import { LocaleContext, useI18n, type Locale } from "./ui";
import { Channel } from "./collaboration/channel";
import { Graph } from "./collaboration/graph";
const Configuration = lazy(() =>
  import("./collaboration/settings").then((module) => ({
    default: module.Configuration,
  })),
);
const TransactionSettings = lazy(() =>
  import("./collaboration/settings").then((module) => ({
    default: module.TransactionSettings,
  })),
);
import type { Selection } from "./collaboration/details";
const OperationsDialog = lazy(() =>
  import("./collaboration/details").then((module) => ({
    default: module.OperationsDialog,
  })),
);
import { collaborationCopy } from "./collaboration/copy";
import {
  chooseChannel,
  parseQuery,
  resolveLocation,
  stringifyQuery,
  type Destination,
  type SettingsSection,
} from "./collaboration/model";
import "./style.css";
import "./collaboration/style.css";

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, staleTime: 1000 } },
});
type Search = { channel?: string; focus?: string; view?: string };
type BrowserSession = {
  id: string;
  operator: boolean;
  mappings: { id: string; tenant: string; subject: string }[];
};
type Registration = { status: string; expires_at: string } | null;
const authCopy = {
  "ja-JP": {
    signIn: "Keycloak でサインイン",
    setup:
      "管理者が Aidash の OIDC 接続を設定してください。API の Bearer 認証は引き続き利用できます。",
    choose: "このタブで使う権限を選択してください",
    operator: "operator",
    request: "登録を申請",
    pending: "登録申請は承認待ちです。",
    rejected: "登録申請は却下されました。24 時間後に再申請できます。",
    expired: "登録申請の期限が切れました。再申請できます。",
    allDevices: "全端末からログアウト",
    currentDevice: "このブラウザからログアウト",
  },
  "en-US": {
    signIn: "Sign in with Keycloak",
    setup:
      "Ask an administrator to configure OIDC for Aidash. Bearer API access remains available.",
    choose: "Choose the authority for this tab",
    operator: "operator",
    request: "Request access",
    pending: "Your registration is awaiting approval.",
    rejected: "Your request was rejected. You can try again after 24 hours.",
    expired: "Your request expired. You can submit another.",
    allDevices: "Log out on all devices",
    currentDevice: "Log out of this browser",
  },
} as const;
function App() {
  const [locale, setLocale] = useState<Locale>(() =>
    localStorage.getItem("aidash-locale") === "en-US" ? "en-US" : "ja-JP",
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
  setLocale: (locale: Locale) => void;
}) {
  const { t } = useI18n();
  const copy = collaborationCopy[locale];
  const location = useLocation();
  const navigate = useNavigate();
  const route = resolveLocation(location.pathname, location.searchStr);
  const client = useQueryClient();
  const [authLoading, setAuthLoading] = useState(true);
  const [oidcEnabled, setOidcEnabled] = useState(false);
  const [browserSession, setBrowserSession] = useState<BrowserSession | null>(
    null,
  );
  const [context, setContext] = useState<string | null>(null);
  const connected = browserSession !== null && context !== null;
  const auth = authCopy[locale];
  const [streamStatus, setStreamStatus] = useState<"live" | "reconnecting">(
    "reconnecting",
  );
  const [filter, setFilter] = useState("");
  const [selection, setSelection] = useState<Selection | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [mobileChannels, setMobileChannels] = useState(false);
  const registration = useQuery({
    queryKey: ["registration", browserSession?.id],
    queryFn: async (): Promise<Registration> => {
      const response = await fetch("/auth/registration", { cache: "no-store" });
      if (!response.ok) throw new Error("Unable to read registration status");
      return response.json() as Promise<Registration>;
    },
    enabled: browserSession !== null,
    refetchInterval: 60_000,
  });
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
  const operator = session.data?.access.kind === "operator";
  const mesh = useQuery({
    queryKey: ["mesh"],
    queryFn: () => getMesh(),
    enabled: connected && operator,
    refetchInterval: 3000,
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
    enabled:
      connected &&
      operator &&
      route.section === "settings" &&
      route.settings === "marketplace",
  });
  const go = (
    section: Destination,
    context: {
      channel?: string;
      focus?: string;
      settings?: SettingsSection;
    } = {},
  ) => {
    setSelection(null);
    setMobileChannels(false);
    void navigate({
      to: "/$section",
      params: { section },
      search: {
        channel: context.channel ?? route.channel,
        focus:
          section !== "settings" ? (context.focus ?? route.focus) : undefined,
        view:
          section === "settings"
            ? (context.settings ?? route.settings)
            : undefined,
      },
    });
  };
  useEffect(() => {
    let cancelled = false;
    sessionStorage.removeItem("aidash-token");
    const restore = async () => {
      try {
        const configResponse = await fetch("/auth/config", {
          cache: "no-store",
        });
        if (!configResponse.ok)
          throw new Error("Aidash authentication is unavailable");
        const config = (await configResponse.json()) as { enabled: boolean };
        if (cancelled) return;
        setOidcEnabled(config.enabled);
        if (!config.enabled) return;
        const response = await fetch("/auth/session", { cache: "no-store" });
        if (response.status === 401) return;
        if (!response.ok) throw new Error("Unable to read browser session");
        const session = (await response.json()) as BrowserSession;
        if (cancelled) return;
        const previousSession = sessionStorage.getItem("aidash-session-id");
        if (previousSession !== session.id) selectDashboardContext(null);
        sessionStorage.setItem("aidash-session-id", session.id);
        const selected = dashboardContext();
        const valid =
          selected === "operator"
            ? session.operator
            : session.mappings.some(
                (mapping) => selected === `mapping:${mapping.id}`,
              );
        setBrowserSession(session);
        setContext(valid ? selected : null);
        if (!valid) selectDashboardContext(null);
      } catch (reason) {
        if (!cancelled)
          setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (!cancelled) setAuthLoading(false);
      }
    };
    void restore();
    return () => {
      cancelled = true;
    };
  }, []);
  const clearBrowser = () => {
    selectDashboardContext(null);
    sessionStorage.removeItem("aidash-session-id");
    setContext(null);
    setBrowserSession(null);
    setSelection(null);
    client.clear();
  };
  const chooseContext = (selected: string) => {
    void client.cancelQueries();
    client.clear();
    selectDashboardContext(selected);
    setContext(selected);
    setStreamStatus("reconnecting");
    setSelection(null);
  };
  const logOut = async (allDevices: boolean) => {
    const csrf = csrfToken();
    if (!csrf) {
      setError("Missing CSRF token");
      return;
    }
    try {
      const response = await fetch(
        allDevices ? "/auth/logout-all" : "/auth/logout",
        {
          method: "POST",
          headers: { "x-aidash-csrf": csrf },
          credentials: "same-origin",
        },
      );
      if (!response.ok) throw new Error("Logout failed");
      clearBrowser();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };
  const disconnect = () => {
    void logOut(false);
  };
  useEffect(() => {
    const revoked = (event: Event) => {
      setError((event as CustomEvent<string>).detail);
      selectDashboardContext(null);
      sessionStorage.removeItem("aidash-session-id");
      setContext(null);
      setBrowserSession(null);
      setSelection(null);
      client.clear();
    };
    window.addEventListener(AUTHENTICATION_EXPIRED, revoked);
    return () => window.removeEventListener(AUTHENTICATION_EXPIRED, revoked);
  }, [client]);
  const browserSessionId = browserSession?.id;
  useEffect(() => {
    if (!browserSessionId) return;
    let lastSent = 0;
    const onActivity = () => {
      if (Date.now() - lastSent < 15_000) return;
      const csrf = csrfToken();
      if (!csrf) return;
      lastSent = Date.now();
      void fetch("/auth/activity", {
        method: "POST",
        credentials: "same-origin",
        headers: { "x-aidash-csrf": csrf },
      });
    };
    window.addEventListener("pointerdown", onActivity);
    window.addEventListener("keydown", onActivity);
    return () => {
      window.removeEventListener("pointerdown", onActivity);
      window.removeEventListener("keydown", onActivity);
    };
  }, [browserSessionId]);
  useEffect(() => {
    if (!browserSessionId) return;
    let cancelled = false;
    const refresh = async () => {
      try {
        const response = await fetch("/auth/session", { cache: "no-store" });
        if (cancelled) return;
        if (response.status === 401 || response.status === 403) {
          selectDashboardContext(null);
          sessionStorage.removeItem("aidash-session-id");
          setContext(null);
          setBrowserSession(null);
          client.clear();
          return;
        }
        if (!response.ok) return;
        const current = (await response.json()) as BrowserSession;
        if (cancelled) return;
        if (current.id !== browserSessionId) {
          void client.cancelQueries();
          selectDashboardContext(null);
          sessionStorage.setItem("aidash-session-id", current.id);
          setContext(null);
          setSelection(null);
          client.clear();
          setBrowserSession(current);
          return;
        }
        const selected = dashboardContext();
        const valid =
          selected === "operator"
            ? current.operator
            : current.mappings.some(
                (mapping) => selected === `mapping:${mapping.id}`,
              );
        if (!valid) {
          void client.cancelQueries();
          selectDashboardContext(null);
          setContext(null);
          client.clear();
        }
        setBrowserSession(current);
      } catch {
        /* The next request still checks authority on the server. */
      }
    };
    const timer = window.setInterval(() => {
      void refresh();
    }, 60_000);
    const onVisible = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [browserSessionId, client]);
  useEffect(() => {
    if (!connected) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    void subscribe(
      controller.signal,
      () => {
        if (timer) return;
        timer = setTimeout(() => {
          for (const key of [
            "state",
            "workspace",
            "channel-history",
            "run",
            "mesh",
            "packages",
            "generation",
          ]) {
            void client.invalidateQueries({ queryKey: [key] });
          }
          timer = undefined;
        }, 250);
      },
      setStreamStatus,
    );
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [connected, client, context]);
  useEffect(() => {
    if (route.legacy) {
      void navigate({
        to: "/$section",
        params: { section: route.section },
        search: {
          channel: route.channel || undefined,
          focus: route.focus || undefined,
          view: route.section === "settings" ? route.settings : undefined,
        },
        replace: true,
      });
    }
  }, [
    route.legacy,
    route.section,
    route.settings,
    route.channel,
    route.focus,
    navigate,
  ]);
  const open = (value: Selection) => {
    setError("");
    setSelection(value);
  };
  const submit = async (request: () => Promise<unknown>): Promise<boolean> => {
    if (busyRef.current) return false;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      const result = await request();
      await client.invalidateQueries();
      if (selection?.kind === "workspace" || selection?.kind === "goal") {
        const value = result as { id?: string; workspace?: { id?: string } };
        const id = value?.workspace?.id ?? value?.id;
        if (id) go("collaboration", { channel: id });
      }
      setSelection(null);
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
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
          <h1>{browserSession ? auth.choose : t("connect")}</h1>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {authLoading ? (
            <p>Loading…</p>
          ) : !oidcEnabled ? (
            <p>{auth.setup}</p>
          ) : !browserSession ? (
            <button
              className="primary"
              type="button"
              onClick={() => {
                const returnTo =
                  window.location.pathname + window.location.search;
                window.location.assign(
                  `/auth/login?return_to=${encodeURIComponent(returnTo)}`,
                );
              }}
            >
              {auth.signIn}
            </button>
          ) : (
            <>
              {browserSession.mappings.map((mapping) => (
                <button
                  key={mapping.id}
                  type="button"
                  onClick={() => chooseContext(`mapping:${mapping.id}`)}
                >
                  {mapping.tenant} / {mapping.subject}
                </button>
              ))}
              {browserSession.operator && (
                <button type="button" onClick={() => chooseContext("operator")}>
                  {auth.operator}
                </button>
              )}
              {browserSession.mappings.length === 0 &&
                !browserSession.operator && (
                  <>
                    <p>
                      {registration.data?.status === "pending"
                        ? auth.pending
                        : registration.data?.status === "rejected"
                          ? auth.rejected
                          : registration.data?.status === "expired"
                            ? auth.expired
                            : auth.choose}
                    </p>
                    {registration.data?.status !== "pending" && (
                      <button
                        type="button"
                        onClick={() => {
                          const csrf = csrfToken();
                          if (!csrf) {
                            setError("Missing CSRF token");
                            return;
                          }
                          void fetch("/auth/registration", {
                            method: "POST",
                            credentials: "same-origin",
                            headers: { "x-aidash-csrf": csrf },
                          })
                            .then(async (response) => {
                              if (!response.ok)
                                throw new Error(
                                  (
                                    (await response.json()) as {
                                      error?: string;
                                    }
                                  ).error ?? "Registration failed",
                                );
                              void registration.refetch();
                            })
                            .catch((reason: unknown) =>
                              setError(
                                reason instanceof Error
                                  ? reason.message
                                  : String(reason),
                              ),
                            );
                        }}
                      >
                        {auth.request}
                      </button>
                    )}
                  </>
                )}
              <button type="button" onClick={disconnect}>
                {auth.currentDevice}
              </button>
              <button
                type="button"
                onClick={() => {
                  void logOut(true);
                }}
              >
                {auth.allDevices}
              </button>
            </>
          )}
        </div>
      </main>
    );
  const data = state.isError ? undefined : state.data;
  const remote = mesh.isError ? undefined : mesh.data;
  const selectedRef = data
    ? chooseChannel(data.workspaces, route.channel)
    : undefined;
  const workspace =
    selectedRef &&
    data?.workspaces.find((value) => value.id === selectedRef.id);
  const currentChannel = workspace?.id ?? route.channel;
  const runs = data
    ? [
        ...data.runs.map((run) => ({ run, node: data.node.id })),
        ...(operator
          ? (remote?.nodes.flatMap((node) =>
              node.runs.map((run) => ({ run, node: node.node_id })),
            ) ?? [])
          : []),
      ]
    : [];
  const requests = data
    ? [
        ...data.human_requests.map((request) => ({
          request,
          node: data.node.id,
        })),
        ...(operator
          ? (remote?.nodes.flatMap((node) =>
              node.human_requests.map((request) => ({
                request,
                node: node.node_id,
              })),
            ) ?? [])
          : []),
      ]
    : [];
  return (
    <DisplayProvider data={data}>
      <div className="collab-app">
        <aside className="collab-rail">
          <div className="brand">
            <span className="brand-symbol">a</span>
            <strong>Aidash</strong>
          </div>
          <nav aria-label={copy.collaboration}>
            {(
              [
                ["collaboration", MessageSquare],
                ["graph", Network],
              ] as const
            ).map(([name, Icon]) => (
              <Link
                key={name}
                to="/$section"
                params={{ section: name }}
                search={{
                  channel: currentChannel || undefined,
                  focus:
                    name === "graph" ? route.focus || undefined : undefined,
                }}
                aria-current={route.section === name ? "page" : undefined}
                className={`collab-nav ${route.section === name ? "selected" : ""}`}
              >
                <Icon size={20} />
                <span>{copy[name]}</span>
              </Link>
            ))}
          </nav>
          <nav className="collab-secondary" aria-label={copy.settings}>
            <Link
              to="/$section"
              params={{ section: "settings" }}
              search={{
                channel: currentChannel || undefined,
                view: route.settings,
              }}
              aria-current={route.section === "settings" ? "page" : undefined}
              className={`collab-nav ${route.section === "settings" ? "selected" : ""}`}
            >
              <Settings size={20} />
              <span>{copy.settings}</span>
            </Link>
          </nav>
        </aside>
        <div className="collab-shell">
          <header className="collab-topbar">
            {route.section === "collaboration" && (
              <button
                type="button"
                className="collab-channel-toggle"
                aria-expanded={mobileChannels}
                onClick={() => setMobileChannels((value) => !value)}
              >
                {copy.channels}
              </button>
            )}
            <span className={`stream-status ${streamStatus}`}>
              <span className="status-dot" />
              {copy[streamStatus]}
            </span>
            <label>
              <span className="sr-only">{auth.choose}</span>
              <select
                value={context ?? ""}
                onChange={(event) => chooseContext(event.target.value)}
              >
                {browserSession?.mappings.map((mapping) => (
                  <option key={mapping.id} value={`mapping:${mapping.id}`}>
                    {mapping.tenant} / {mapping.subject}
                  </option>
                ))}
                {browserSession?.operator && (
                  <option value="operator">{auth.operator}</option>
                )}
              </select>
            </label>
            <label>
              <span className="sr-only">{t("language")}</span>
              <select
                data-testid="language-selector"
                value={locale}
                onChange={(event) => setLocale(event.target.value as Locale)}
              >
                <option value="ja-JP">日本語</option>
                <option value="en-US">English</option>
              </select>
            </label>
            <button type="button" onClick={disconnect}>
              {auth.currentDevice}
            </button>
            <button
              type="button"
              onClick={() => {
                void logOut(true);
              }}
            >
              {auth.allDevices}
            </button>
          </header>
          <div
            className={`collab-workspace ${route.section !== "collaboration" ? "wide" : ""}`}
          >
            {route.section === "collaboration" && (
              <aside
                className={`collab-channel-sidebar ${mobileChannels ? "is-open" : ""}`}
                aria-label={copy.channels}
              >
                <div className="collab-sidebar-title">
                  <h2>{copy.channels}</h2>
                  <button
                    type="button"
                    aria-label={copy.create}
                    onClick={() => open({ kind: "workspace" })}
                  >
                    <Plus size={18} />
                  </button>
                </div>
                <label>
                  <span className="sr-only">{copy.search}</span>
                  <input
                    type="search"
                    value={filter}
                    onChange={(event) => setFilter(event.target.value)}
                    placeholder={copy.search}
                  />
                </label>
                <nav aria-label={copy.channels}>
                  {data?.workspaces
                    .filter((value) =>
                      value.title
                        .toLocaleLowerCase()
                        .includes(filter.toLocaleLowerCase()),
                    )
                    .map((value) => (
                      <Link
                        key={value.id}
                        to="/$section"
                        params={{ section: "collaboration" }}
                        search={{ channel: value.id }}
                        aria-label={`# ${value.title}`}
                        className={`collab-channel-link ${workspace?.id === value.id ? "selected" : ""}`}
                        aria-current={
                          workspace?.id === value.id ? "page" : undefined
                        }
                        onClick={() => {
                          setMobileChannels(false);
                          setSelection(null);
                        }}
                      >
                        <span aria-hidden="true">#</span> {value.title}
                        {requests.some(
                          (item) =>
                            item.request.workspace_id === value.id &&
                            item.request.response === null,
                        ) && (
                          <span
                            className="collab-unread"
                            aria-label={copy.needsInput}
                          >
                            !
                          </span>
                        )}
                      </Link>
                    ))}
                </nav>
                <div className="collab-start-actions">
                  <button
                    type="button"
                    onClick={() => open({ kind: "workspace" })}
                  >
                    {copy.prepare}
                  </button>
                  <button type="button" onClick={() => open({ kind: "goal" })}>
                    {copy.createAndStart}
                  </button>
                </div>
              </aside>
            )}
            <main className="collab-main">
              {error && !selection && (
                <p className="error" role="alert">
                  {error}
                </p>
              )}
              {state.isError && (
                <div className="error" role="alert">
                  <p>{state.error.message}</p>
                  <button type="button" onClick={() => void state.refetch()}>
                    {copy.retry}
                  </button>
                </div>
              )}
              {!data && !state.isError && (
                <p role="status">{copy.processing}</p>
              )}
              {data && (
                <>
                  {route.section === "collaboration" &&
                    (workspace ? (
                      <Channel
                        key={workspace.id}
                        workspace={workspace}
                        data={data}
                        runs={runs}
                        requests={requests}
                        open={open}
                        graph={(focus) =>
                          go("graph", {
                            channel: workspace.id,
                            focus: focus ?? "",
                          })
                        }
                      />
                    ) : (
                      <section className="collab-welcome">
                        <h1>
                          {route.channel ? copy.unavailable : copy.noChannel}
                        </h1>
                        <p>{copy.channelHelp}</p>
                        <button
                          type="button"
                          className="primary"
                          onClick={() => open({ kind: "workspace" })}
                        >
                          {copy.create}
                        </button>
                      </section>
                    ))}
                  {route.section === "graph" && (
                    <Graph
                      data={data}
                      discovery={discovery.isError ? undefined : discovery.data}
                      runs={runs}
                      channel={currentChannel}
                      focus={route.focus}
                      setFocus={(focus) => go("graph", { focus })}
                      visitChannel={(channel) =>
                        go("collaboration", { channel })
                      }
                      open={open}
                    />
                  )}
                  {route.section === "settings" &&
                    (route.settings !== "transactions" || !operator) && (
                      <Suspense
                        fallback={<p role="status">{copy.processing}</p>}
                      >
                        <Configuration
                          data={data}
                          section={route.settings}
                          select={(settings) => go("settings", { settings })}
                          open={open}
                          packages={
                            packages.isError ? [] : (packages.data ?? [])
                          }
                          disconnect={disconnect}
                        />
                      </Suspense>
                    )}
                  {operator &&
                    (mesh.isError || (remote?.errors.length ?? 0) > 0) && (
                      <p className="notice" role="status">
                        {t("remoteUnavailable")}
                        {remote?.errors.map((peer) => (
                          <span key={peer.node_id}>
                            <ReferenceName id={peer.node_id} />: {peer.error}
                          </span>
                        ))}
                      </p>
                    )}
                </>
              )}
              {operator &&
                session.data &&
                route.section === "settings" &&
                route.settings === "transactions" && (
                  <Suspense fallback={<p role="status">{copy.processing}</p>}>
                    <TransactionSettings
                      nodeId={session.data.node_id}
                      select={(settings) => go("settings", { settings })}
                    />
                  </Suspense>
                )}
            </main>
          </div>
        </div>
        {selection && data && (
          <Suspense fallback={<p role="status">{copy.processing}</p>}>
            <OperationsDialog
              selection={selection}
              data={data}
              discovery={discovery.isError ? undefined : discovery.data}
              mesh={remote}
              open={open}
              close={() => setSelection(null)}
              submit={submit}
              error={error}
              busy={busy}
              visitChannel={(channel) => go("collaboration", { channel })}
            />
          </Suspense>
        )}
      </div>
    </DisplayProvider>
  );
}
const rootRoute = createRootRoute({
  component: App,
  validateSearch: (search: Record<string, unknown>): Search => ({
    channel: typeof search.channel === "string" ? search.channel : undefined,
    focus: typeof search.focus === "string" ? search.focus : undefined,
    view: typeof search.view === "string" ? search.view : undefined,
  }),
});
const indexRoute = createRoute({ getParentRoute: () => rootRoute, path: "/" });
const sectionRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/$section",
});
const router = createRouter({
  routeTree: rootRoute.addChildren([indexRoute, sectionRoute]),
  parseSearch: parseQuery,
  stringifySearch: stringifyQuery,
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
