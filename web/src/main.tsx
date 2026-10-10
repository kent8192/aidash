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
import { RequestWelcome } from "./collaboration/request/welcome";
import { Graph } from "./collaboration/graph";
import { Workbench } from "./workbench";
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
import {
  ConversationTools,
  CreatorTools,
  TrustTools,
} from "./integrated-tools";
import { Button } from "./components/ui/button";
import { TooltipProvider } from "./components/ui/tooltip";
import { NotificationProvider } from "./notifications";
import type { NoticeTarget } from "./notifications/model";
import {
  storedThemePreference,
  useTheme,
  type Theme,
  type ThemePreference,
} from "./theme";
import { AppShell } from "./shell/app-shell";
import { AuthCard } from "./shell/auth-card";
import { Alert, Loading, Notice } from "./components/patterns";
import { authCopy, shellCopy } from "./shell/copy";

const GcipSignIn = lazy(() => import("./gcip-sign-in"));

function SignInEntry({
  locale,
  setLocale,
}: {
  locale: Locale;
  setLocale: (locale: Locale) => void;
}) {
  const configuration = useQuery({
    queryKey: ["sign-in-configuration"],
    queryFn: async () => {
      const response = await fetch("/auth/config", { cache: "no-store" });
      if (!response.ok) throw new Error();
      return response.json() as Promise<{
        enabled: boolean;
        provider?: string;
      }>;
    },
  });
  if (configuration.data?.enabled && configuration.data.provider === "gcip")
    return (
      <Suspense
        fallback={
          <AuthCard title="Aidash">
            <Loading>{shellCopy[locale].loading}</Loading>
          </AuthCard>
        }
      >
        <GcipSignIn locale={locale} setLocale={setLocale} />
      </Suspense>
    );
  return (
    <AuthCard title="Aidash">
      <p role="status" className="text-muted-foreground">
        {configuration.isLoading
          ? shellCopy[locale].loading
          : authCopy[locale].setup}
      </p>
    </AuthCard>
  );
}

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
function App() {
  const [locale, setLocale] = useState<Locale>(() =>
    localStorage.getItem("aidash-locale") === "en-US" ? "en-US" : "ja-JP",
  );
  const { preference, theme, setPreference } = useTheme();
  useEffect(() => {
    document.documentElement.lang = locale;
    localStorage.setItem("aidash-locale", locale);
  }, [locale]);
  return (
    <LocaleContext value={locale}>
      <div className="flex h-dvh min-h-0 flex-col bg-background text-foreground">
        {window.location.pathname === "/sign-in" ? (
          <SignInEntry locale={locale} setLocale={setLocale} />
        ) : (
          <ConnectionGate english={locale === "en-US"}>
            <Dashboard
              locale={locale}
              setLocale={setLocale}
              theme={theme}
              preference={preference}
              setPreference={setPreference}
            />
          </ConnectionGate>
        )}
      </div>
    </LocaleContext>
  );
}
function Dashboard({
  locale,
  setLocale,
  theme,
  preference,
  setPreference,
}: {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  theme: Theme;
  preference: ThemePreference;
  setPreference: (preference: ThemePreference) => void;
}) {
  const { t } = useI18n();
  const copy = collaborationCopy[locale];
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
  const [selection, setSelection] = useState<Selection | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
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
            "provider-credentials",
            "provider-credential-bindings",
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
      <AuthCard title={browserSession ? auth.choose : t("connect")}>
        {error && <Alert>{error}</Alert>}
        {authLoading ? (
          <Loading>{shellCopy[locale].loading}</Loading>
        ) : !oidcEnabled ? (
          <p className="text-muted-foreground">{auth.setup}</p>
        ) : !browserSession ? (
          <Button
            size="lg"
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
            {oidcProvider === "gcip"
              ? locale === "ja-JP"
                ? "サインイン"
                : "Sign in"
              : oidcProvider === "keycloak"
                ? auth.signIn.replace("Google", "Keycloak")
                : auth.signIn}
          </Button>
        ) : (
          <>
            {(browserSession.mappings.length > 0 ||
              browserSession.operator) && (
              <div className="grid gap-1.5">
                {browserSession.mappings.map((mapping) => (
                  <Button
                    variant="outline"
                    size="lg"
                    key={mapping.id}
                    type="button"
                    className="justify-start font-mono text-xs"
                    onClick={() => chooseContext(`mapping:${mapping.id}`)}
                  >
                    {mapping.tenant} / {mapping.subject}
                  </Button>
                ))}
                {browserSession.operator && (
                  <Button
                    variant="outline"
                    size="lg"
                    type="button"
                    className="justify-start font-mono text-xs"
                    onClick={() => chooseContext("operator")}
                  >
                    {auth.operator}
                  </Button>
                )}
              </div>
            )}
            {browserSession.mappings.length === 0 &&
              !browserSession.operator && (
                <div className="grid gap-3">
                  <p
                    className={
                      registration.data?.status === "pending"
                        ? "rounded-md bg-warning-soft px-3 py-2 text-xs text-warning"
                        : registration.data?.status === "rejected" ||
                            registration.data?.status === "expired"
                          ? "rounded-md bg-destructive-soft px-3 py-2 text-xs text-destructive"
                          : "text-muted-foreground"
                    }
                  >
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
                </div>
              )}
            <div className="flex flex-wrap gap-1 border-t border-border pt-3">
              <Button variant="ghost" size="sm" type="button" onClick={disconnect}>
                {auth.currentDevice}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                onClick={() => {
                  void logOut(true);
                }}
              >
                {auth.allDevices}
              </Button>
            </div>
          </>
        )}
      </AuthCard>
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
  const visit = (target: NoticeTarget) => {
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
      data.workspaces.some((workspace) => workspace.id === target.workspace)
    )
      go("collaboration", { channel: target.workspace, focus: "" });
    if (item) open({ kind: "human", ...item });
    if (task) open({ kind: "taskDetail", task });
  };
  return (
    <DisplayProvider data={data}>
      <NotificationProvider
        key={`${browserSessionId}:${context}`}
        data={data}
        remote={operator ? remote : undefined}
        locale={locale}
        theme={theme}
        onSelect={visit}
      >
        <AppShell
          section={route.section}
          settings={route.settings}
          data={data}
          channel={currentChannel}
          operator={operator}
          streamStatus={streamStatus}
          decisions={requests.filter((item) => item.request.response === null)}
          selectDecision={({ request, node }) =>
            visit({
              kind: "human",
              id: request.id,
              node,
              workspace: request.workspace_id,
            })
          }
          go={go}
          newRequest={() => open({ kind: "goal" })}
          prepareChannel={() => open({ kind: "workspace" })}
          theme={theme}
          account={{
            name:
              session.data?.access.kind === "subject"
                ? session.data.access.subject
                : auth.operator,
            context,
            mappings: browserSession?.mappings ?? [],
            operatorAllowed: browserSession?.operator ?? false,
            chooseContext,
            setLocale,
            logOut: (allDevices) => void logOut(allDevices),
            preference,
            setPreference,
          }}
        >
          {((error && !selection) ||
            state.isError ||
            (data &&
              operator &&
              (mesh.isError || (remote?.errors.length ?? 0) > 0))) && (
            <div className="grid shrink-0 gap-2 border-b border-border px-4 py-3 md:px-6">
              {error && !selection && (
                <Alert>{error}</Alert>
              )}
              {state.isError && (
                <Alert
                  retry={() => void state.refetch()}
                  retryLabel={copy.retry}
                >
                  <p>{state.error.message}</p>
                  {session.data && (
                    <Button
                      variant="outline"
                      size="sm"
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
                      {shellCopy[locale].recovery}
                    </Button>
                  )}
                </Alert>
              )}
              {data &&
                operator &&
                (mesh.isError || (remote?.errors.length ?? 0) > 0) && (
                  <Notice tone="warning" role="status">
                    <p>{t("remoteUnavailable")}</p>
                    {remote?.errors.map((peer) => (
                      <p
                        key={peer.node_id}
                        className="font-mono text-[11px] text-muted-foreground"
                      >
                        <ReferenceName id={peer.node_id} />: {peer.error}
                      </p>
                    ))}
                  </Notice>
                )}
            </div>
          )}
          {!data && !state.isError && (
            <Loading className="flex flex-1 items-center justify-center">
              {copy.processing}
            </Loading>
          )}
          {data && (
            <div className="relative flex min-h-0 flex-1 flex-col">
              {route.section === "collaboration" &&
                (workspace ? (
                  <Channel
                    key={workspace.id}
                    workspace={workspace}
                    data={data}
                    discovery={discovery.isError ? undefined : discovery.data}
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
                  <RequestWelcome
                    unavailable={!!route.channel}
                    create={() => open({ kind: "goal" })}
                  />
                ))}
              {route.section === "graph" && (
                <Graph
                  key={`${context}:${currentChannel}`}
                  data={data}
                  discovery={discovery.isError ? undefined : discovery.data}
                  runs={runs}
                  channel={currentChannel}
                  focus={route.focus}
                  setFocus={(focus) => go("graph", { focus })}
                  visitChannel={(channel) => go("collaboration", { channel })}
                  open={open}
                />
              )}
              {(route.section === "creator" || route.section === "trust") && (
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
                        initiallyOpen={route.integration === "authorization"}
                      />
                    )
                  }
                />
              )}
              {route.section === "settings" && (
                <Suspense
                  fallback={
                    <Loading className="p-6">{copy.processing}</Loading>
                  }
                >
                  <Configuration
                    data={data}
                    section={route.settings}
                    select={(settings) => go("settings", { settings })}
                    open={open}
                    packages={packages.isError ? [] : (packages.data ?? [])}
                    disconnect={disconnect}
                    integration={route.integration}
                    channel={currentChannel}
                  />
                </Suspense>
              )}
            </div>
          )}
          {!data && session.data && route.section === "trust" && (
            <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4 md:px-6">
              <TrustTools
                entries={[]}
                operator={operator}
                initiallyOpen={route.integration === "authorization"}
              />
            </div>
          )}
        </AppShell>
        {session.data && (
          <ConversationTools
            key={`${context}:${route.integration}`}
            view={
              route.integration === "files" || route.integration === "progress"
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
          <Suspense fallback={<Loading>{copy.processing}</Loading>}>
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
// Resolve the theme before the first render so the stored preference never flashes.
const initialTheme = storedThemePreference();
document.documentElement.dataset.theme =
  initialTheme === "system"
    ? window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light"
    : initialTheme;
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={300}>
        <RouterProvider router={router} />
      </TooltipProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
