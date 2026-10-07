import { lazy, Suspense, useEffect, useRef, useState } from "react";
import React from "react";
import { createRoot } from "react-dom/client";
import { DisplayProvider, ReferenceName } from "./record-view";
import {
  createRootRoute,
  createRoute,
  createRouter,
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
import { Search, ChevronDown, PanelLeft, Moon, Sun } from "lucide-react";
import aidashLogo from "./assets/brand/aidash-logo.svg?no-inline";
import { workspaceCopy } from "./collaboration/workspace-copy";
import { Avatar } from "./collaboration/avatar";
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
  sessionFetch,
  signIn,
  signOut,
  dashboardContext,
  selectDashboardContext,
} from "./transport";
import { ConnectionGate } from "./connection-gate";
import { LocaleContext, useI18n, type Locale } from "./ui";
import { Channel } from "./collaboration/channel";
import { Graph } from "./collaboration/graph";
import { Workbench } from "./workbench";
import { meshCopy } from "./collaboration/mesh-copy";
const Configuration = lazy(() =>
  import("./collaboration/settings").then((module) => ({
    default: module.Configuration,
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
import "./design-system.css";
import "./style.css";
import "./collaboration/style.css";
import "./collaboration/workspace.css";
import "./collaboration/mesh.css";
import "./workbench.css";
import "./trust-overview.css";
import "./intent-ui.css";
import { IntentSidebar } from "./intent-sidebar";
import {
  ConversationTools,
  CreatorTools,
  TrustTools,
} from "./integrated-tools";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NotificationBell, NotificationProvider } from "./notifications";

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
    signIn: "Google でサインイン",
    setup:
      "管理者が Aidash の OIDC 接続を設定してください。API の Bearer 認証は引き続き利用できます。",
    choose: "このタブで使う権限を選択してください",
    operator: "operator",
    request: "登録を申請",
    pending: "登録申請は承認待ちです。",
    rejected: "登録申請は却下されました。24 時間後に再申請できます。",
    expired: "登録申請の期限が切れました。再申請できます。",
    allDevices: "全端末からログアウト",
    currentDevice: "この端末からログアウト",
  },
  "en-US": {
    signIn: "Sign in with Google",
    setup:
      "Ask an administrator to configure OIDC for Aidash. Bearer API access remains available.",
    choose: "Choose the authority for this tab",
    operator: "operator",
    request: "Request access",
    pending: "Your registration is awaiting approval.",
    rejected: "Your request was rejected. You can try again after 24 hours.",
    expired: "Your request expired. You can submit another.",
    allDevices: "Log out on all devices",
    currentDevice: "Log out on this device",
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
      <ConnectionGate english={locale === "en-US"}>
        <Dashboard locale={locale} setLocale={setLocale} />
      </ConnectionGate>
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
  const words = workspaceCopy[locale];
  const searchInput = useRef<HTMLInputElement>(null);
  const location = useLocation();
  const navigate = useNavigate();
  const route = resolveLocation(location.pathname, location.searchStr);
  const client = useQueryClient();
  const [authLoading, setAuthLoading] = useState(true);
  const [oidcEnabled, setOidcEnabled] = useState(false);
  const [oidcProvider, setOidcProvider] = useState("google");
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
  const [graphSearch, setGraphSearch] = useState("");
  const [theme, setTheme] = useState(() =>
    localStorage.getItem("aidash-theme") === "dark" ? "dark" : "light",
  );
  useEffect(() => {
    localStorage.setItem("aidash-theme", theme);
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [mobileChannels, setMobileChannels] = useState(false);
  useEffect(() => {
    const search = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setMobileChannels(true);
        requestAnimationFrame(() => searchInput.current?.focus());
      }
    };
    window.addEventListener("keydown", search);
    return () => window.removeEventListener("keydown", search);
  }, []);
  const registration = useQuery({
    queryKey: ["registration", browserSession?.id],
    queryFn: async (): Promise<Registration> => {
      const response = await sessionFetch("/auth/registration", {
        cache: "no-store",
      });
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
        const configResponse = await sessionFetch("/auth/config", {
          cache: "no-store",
        });
        if (!configResponse.ok)
          throw new Error("Aidash authentication is unavailable");
        const config = (await configResponse.json()) as {
          enabled: boolean;
          provider?: string;
        };
        if (cancelled) return;
        setOidcEnabled(config.enabled);
        setOidcProvider(config.provider ?? "google");
        if (!config.enabled) return;
        const response = await sessionFetch("/auth/session", {
          cache: "no-store",
        });
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
    try {
      await signOut(allDevices);
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
      lastSent = Date.now();
      void sessionFetch("/auth/activity", {
        method: "POST",
        credentials: "same-origin",
      }).catch(() => {
        /* Session polling reports connectivity and revocation. */
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
        const response = await sessionFetch("/auth/session", {
          cache: "no-store",
        });
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
            "marketplace",
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
          view:
            route.integration === "semantic"
              ? "registry"
              : (route.integration ??
                (route.section === "settings" ? route.settings : undefined)),
          ...(route.integration === "semantic" ? { focus: "semantic" } : {}),
        },
        replace: true,
      });
    }
  }, [
    route.legacy,
    route.integration,
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
          <img src={aidashLogo} alt="Aidash" width={172} height={64} />
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
            <Button
              variant="outline"
              className="primary"
              type="button"
              onClick={() => {
                const returnTo =
                  window.location.pathname + window.location.search;
                setAuthLoading(true);
                void signIn(returnTo)
                  .then(async () => {
                    const response = await sessionFetch("/auth/session", {
                      cache: "no-store",
                    });
                    if (!response.ok) throw new Error("Sign-in failed");
                    const current = (await response.json()) as BrowserSession;
                    selectDashboardContext(null);
                    sessionStorage.setItem("aidash-session-id", current.id);
                    setBrowserSession(current);
                  })
                  .catch((reason) => setError(String(reason)))
                  .finally(() => setAuthLoading(false));
              }}
            >
              {oidcProvider === "keycloak"
                ? auth.signIn.replace("Google", "Keycloak")
                : auth.signIn}
            </Button>
          ) : (
            <>
              {browserSession.mappings.map((mapping) => (
                <Button
                  variant="outline"
                  key={mapping.id}
                  type="button"
                  onClick={() => chooseContext(`mapping:${mapping.id}`)}
                >
                  {mapping.tenant} / {mapping.subject}
                </Button>
              ))}
              {browserSession.operator && (
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => chooseContext("operator")}
                >
                  {auth.operator}
                </Button>
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
                      <Button
                        variant="outline"
                        type="button"
                        onClick={() => {
                          void sessionFetch("/auth/registration", {
                            method: "POST",
                            credentials: "same-origin",
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
                      </Button>
                    )}
                  </>
                )}
              <Button variant="outline" type="button" onClick={disconnect}>
                {auth.currentDevice}
              </Button>
              <Button
                variant="outline"
                type="button"
                onClick={() => {
                  void logOut(true);
                }}
              >
                {auth.allDevices}
              </Button>
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
      <NotificationProvider
        key={`${browserSessionId}:${context}`}
        data={data}
        remote={operator ? remote : undefined}
        locale={locale}
        theme={theme === "dark" ? "dark" : "light"}
        onSelect={(target) => {
          const item =
            target.kind === "human"
              ? requests.find(
                  (item) =>
                    item.node === target.node &&
                    item.request.id === target.id &&
                    item.request.response === null,
                )
              : undefined;
          const task =
            target.kind === "task" && target.node === data?.node.id
              ? data.tasks.find((task) => task.id === target.id)
              : undefined;
          if (!item && !task) return;
          if (
            data &&
            target.node === data.node.id &&
            data.workspaces.some(
              (workspace) => workspace.id === target.workspace,
            )
          )
            go("collaboration", { channel: target.workspace, focus: "" });
          if (item) open({ kind: "human", ...item });
          if (task) open({ kind: "taskDetail", task });
        }}
      >
        <div
          className={`collab-app intent-app ${route.section === "graph" ? "graph-shell" : ""}`}
          data-theme={theme}
        >
          <a className="intent-skip" href="#intent-main">
            {locale === "ja-JP" ? "本文へ移動" : "Skip to content"}
          </a>
          <IntentSidebar
            data={data}
            current={currentChannel}
            section={route.section}
            theme={theme}
            filter={filter}
            setFilter={setFilter}
            searchInput={searchInput}
            expanded={mobileChannels}
            close={() => setMobileChannels(false)}
            create={() => open({ kind: "goal" })}
            account={
              <>
                {" "}
                <NotificationBell />{" "}
                <details className="workspace-popover account-popover">
                  <summary aria-label={words.account}>
                    <Avatar
                      name={
                        session.data?.access.kind === "subject"
                          ? session.data.access.subject
                          : "account"
                      }
                      human
                      small
                    />
                    <span>
                      {session.data?.access.kind === "subject"
                        ? session.data.access.subject
                        : auth.operator}
                    </span>
                    <ChevronDown size={12} />
                  </summary>
                  <div className="workspace-popover-body">
                    <nav
                      className="intent-account-links"
                      aria-label={
                        locale === "ja-JP"
                          ? "管理と設定"
                          : "Management and settings"
                      }
                    >
                      <Button
                        variant="outline"
                        type="button"
                        onClick={(event) => {
                          event.currentTarget
                            .closest("details")
                            ?.removeAttribute("open");
                          open({ kind: "workspace" });
                        }}
                      >
                        {copy.prepare}
                      </Button>
                      {(["creator", "trust", "settings"] as const).map(
                        (section) => (
                          <Button
                            variant="outline"
                            type="button"
                            key={section}
                            onClick={(event) => {
                              event.currentTarget
                                .closest("details")
                                ?.removeAttribute("open");
                              go(section, { focus: "" });
                            }}
                          >
                            {section === "settings"
                              ? copy.settings
                              : section === "creator"
                                ? "Creator"
                                : "Trust"}
                          </Button>
                        ),
                      )}
                    </nav>

                    <label>
                      <span className="sr-only">{auth.choose}</span>
                      <select
                        value={context ?? ""}
                        onChange={(event) => chooseContext(event.target.value)}
                      >
                        {browserSession?.mappings.map((mapping) => (
                          <option
                            key={mapping.id}
                            value={`mapping:${mapping.id}`}
                          >
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
                        onChange={(event) =>
                          setLocale(event.target.value as Locale)
                        }
                      >
                        <option value="ja-JP">日本語</option>
                        <option value="en-US">English</option>
                      </select>
                    </label>
                    <Button
                      variant="outline"
                      type="button"
                      onClick={disconnect}
                    >
                      {auth.currentDevice}
                    </Button>
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => {
                        void logOut(true);
                      }}
                    >
                      {auth.allDevices}
                    </Button>
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() =>
                        setTheme(theme === "dark" ? "light" : "dark")
                      }
                    >
                      {theme === "dark" ? (
                        <Sun size={16} />
                      ) : (
                        <Moon size={16} />
                      )}
                      {locale === "ja-JP"
                        ? theme === "dark"
                          ? "ライトテーマ"
                          : "ダークテーマ"
                        : theme === "dark"
                          ? "Light theme"
                          : "Dark theme"}
                    </Button>
                  </div>
                </details>
              </>
            }
          />
          <div className="collab-shell">
            <header
              className={`collab-topbar intent-topbar ${route.section === "collaboration" ? "conversation-topbar" : ""}`}
            >
              <Button
                variant="ghost"
                size="icon"
                className="intent-history-toggle"
                aria-label={copy.channels}
                aria-expanded={mobileChannels}
                onClick={() => setMobileChannels((value) => !value)}
              >
                <PanelLeft size={18} />
              </Button>
              <span className="intent-location">
                {route.section === "collaboration"
                  ? locale === "ja-JP"
                    ? "依頼"
                    : "Request"
                  : route.section === "settings"
                    ? copy.settings
                    : route.section === "graph"
                      ? copy.graph
                      : route.section === "creator"
                        ? "Creator"
                        : "Trust"}
              </span>
              {route.section === "graph" && (
                <label className="graph-search">
                  <Search size={15} />
                  <span className="sr-only">{meshCopy[locale].search}</span>
                  <Input
                    type="search"
                    value={graphSearch}
                    onChange={(event) => setGraphSearch(event.target.value)}
                    placeholder={meshCopy[locale].search}
                  />
                </label>
              )}
              <span className={`stream-status ${streamStatus}`}>
                <span className="status-dot" />
                {copy[streamStatus]}
              </span>
            </header>
            <div
              className={`collab-workspace ${route.section !== "collaboration" ? "wide" : ""}`}
            >
              <main id="intent-main" className="collab-main" tabIndex={-1}>
                {error && !selection && (
                  <p className="error" role="alert">
                    {error}
                  </p>
                )}
                {state.isError && (
                  <div className="error" role="alert">
                    <p>{state.error.message}</p>
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => void state.refetch()}
                    >
                      {copy.retry}
                    </Button>
                    {session.data && (
                      <Button
                        variant="outline"
                        type="button"
                        onClick={() => {
                          void navigate({
                            to: "/$section",
                            params: { section: "collaboration" },
                            search: {
                              channel: currentChannel || undefined,
                              view: "progress",
                            },
                          });
                        }}
                      >
                        {locale === "ja-JP"
                          ? "整合性と復旧"
                          : "Consistency and recovery"}
                      </Button>
                    )}
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
                          discovery={
                            discovery.isError ? undefined : discovery.data
                          }
                          threadList={location.search.view === "threads"}
                          tools={(view) => {
                            void navigate({
                              to: "/$section",
                              params: { section: "collaboration" },
                              search: { channel: workspace.id, view },
                            });
                          }}
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
                            {route.channel
                              ? copy.unavailable
                              : locale === "ja-JP"
                                ? "今日は何を進めますか？"
                                : "What would you like to work on?"}
                          </h1>
                          <p>
                            {route.channel
                              ? copy.channelHelp
                              : locale === "ja-JP"
                                ? "やりたいことを伝えてください。エージェントと一緒に進められます。"
                                : "Describe your goal and work through it with your agents."}
                          </p>
                          <Button
                            variant="outline"
                            type="button"
                            className="primary"
                            onClick={() => open({ kind: "goal" })}
                          >
                            {locale === "ja-JP" ? "新しい依頼" : "New request"}
                          </Button>
                        </section>
                      ))}
                    {route.section === "graph" && (
                      <Graph
                        key={`${context}:${currentChannel}`}
                        data={data}
                        search={graphSearch}
                        setSearch={setGraphSearch}
                        discovery={
                          discovery.isError ? undefined : discovery.data
                        }
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
                    {(route.section === "creator" ||
                      route.section === "trust") && (
                      <Workbench
                        key={context ?? ""}
                        mode={route.section}
                        data={data}
                        focus={route.focus}
                        select={(focus) => go(route.section, { focus })}
                        switchMode={(section, focus) => go(section, { focus })}
                        integratedTools={
                          route.section === "creator" ? (
                            <CreatorTools
                              data={data}
                              initiallyOpen={route.integration === "generation"}
                            />
                          ) : (
                            <TrustTools
                              entries={data.registry}
                              operator={operator}
                              initiallyOpen={
                                route.integration === "authorization"
                              }
                            />
                          )
                        }
                      />
                    )}
                    {route.section === "settings" && (
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
                          integration={route.integration}
                          channel={currentChannel}
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
                {!data && session.data && route.section === "trust" && (
                  <TrustTools
                    entries={[]}
                    operator={operator}
                    initiallyOpen={route.integration === "authorization"}
                  />
                )}
              </main>
            </div>
          </div>
          {session.data && (
            <ConversationTools
              key={`${context}:${route.integration}`}
              view={
                route.integration === "files" ||
                route.integration === "progress"
                  ? route.integration
                  : undefined
              }
              data={data}
              stateError={state.isError ? state.error.message : undefined}
              nodeId={session.data.node_id}
              operator={operator}
              workspace={currentChannel || undefined}
              close={() => go("collaboration", { focus: "" })}
            />
          )}
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
      </NotificationProvider>
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
