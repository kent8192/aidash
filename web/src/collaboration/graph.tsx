import { Button } from "../components/ui/button";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Clock3, Filter, Focus, List, Network, RotateCcw } from "lucide-react";
import type { Discovery, Run, State } from "../types";
import { useI18n } from "../ui";
import { disambiguateLabels } from "../display-labels";
import {
  graphError,
  graphPage,
  graphPeers,
  mergeGraphPages,
  validateGraphPage,
  type GraphPage,
  type RemoteExpansion,
} from "./federated-graph";
import { mergeGraphTimeline } from "./graph-timeline";
import { meshCopy } from "./mesh-copy";
import { MeshIcon } from "./mesh-icons";
import {
  buildMeshGraph,
  eventReferences,
  filterMeshGraph,
  inWindow,
  meshKinds,
  modeKinds,
  type MeshKind,
  type MeshMode,
  type MeshNode,
  type MeshRelation,
} from "./mesh-model";
import { workspaceGraph } from "./graph-regions";
import { MeshInspector } from "./mesh-inspector";
import type { MeshLayout } from "./mesh-canvas";
import type { Selection } from "./details";

const MeshCanvas = lazy(() =>
  import("./mesh-canvas").then((m) => ({ default: m.MeshCanvas })),
);
const Neighborhood = lazy(() =>
  import("./agent-neighborhood").then((m) => ({ default: m.Graph })),
);
const supportedRelations = [
  "contains",
  "goal",
  "depends",
  "produces",
  "executes",
  "tool",
  "model",
  "skill",
  "member",
  "hosts",
  "coordinates",
  "participates",
] as const;
const projectionKinds = meshKinds.filter(
  (kind) => kind !== "human" && kind !== "remote",
);

const validGraphTenant = (value: string) =>
  value.length > 0 &&
  new TextEncoder().encode(value).length <= 256 &&
  !/[\s\p{Cc}*]/u.test(value);

export function Graph({
  data,
  channel,
  focus,
  setFocus,
  visitChannel,
  open,
  discovery,
  runs,
  search = "",
  setSearch = () => {},
}: {
  data: State;
  channel: string;
  focus: string;
  discovery?: Discovery;
  runs: { run: Run; node: string }[];
  setFocus: (id: string) => void;
  visitChannel: (id: string) => void;
  open: (selection: Selection) => void;
  search?: string;
  setSearch?: (value: string) => void;
}) {
  const { locale, t } = useI18n();
  const copy = meshCopy[locale];
  const [requestedMode, setMode] = useState<MeshMode | "neighborhood">("mesh");
  const mode = focus ? "neighborhood" : requestedMode;
  const [layout, setLayout] = useState<MeshLayout>("structured");
  const [hours, setHours] = useState(24);
  const [kinds, setKinds] = useState<MeshKind[]>(
    meshKinds.filter(
      (kind) => !["model", "skill", "conversation"].includes(kind),
    ),
  );
  const [relations, setRelations] = useState<MeshRelation[] | undefined>();
  const [selectedId, setSelectedId] = useState(focus);
  const [closed, setClosed] = useState(false);
  const [focused, setFocused] = useState("");
  const [list, setList] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [peerIds, setPeerIds] = useState<string[]>([]);
  const [expansions, setExpansions] = useState<Record<string, RemoteExpansion>>(
    {},
  );
  const [targetTenant, setTargetTenant] = useState("");
  const [graphWorkspace, setGraphWorkspace] = useState(channel);
  const [refreshTick, setRefreshTick] = useState(0);
  const requests = useRef(new Map<string, number>());
  const expansionsRef = useRef(expansions);
  useEffect(() => {
    expansionsRef.current = expansions;
  }, [expansions]);
  useEffect(() => {
    setGraphWorkspace(channel);
  }, [channel]);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const graphMode = mode === "neighborhood" ? "mesh" : mode;
  const expandedKey = Object.keys(expansions).sort().join("\u0000");
  const scope = JSON.stringify([graphWorkspace, hours, targetTenant]);
  useEffect(() => {
    setExpansions((previous) => {
      if (Object.values(previous).every((value) => value.scope === scope))
        return previous;
      return Object.fromEntries(
        Object.entries(previous).map(([peer]) => [
          peer,
          { state: "loading", scope },
        ]),
      );
    });
  }, [scope]);
  useEffect(() => {
    const deadlines = Object.values(expansions)
      .filter((value) => value.page && value.checkedAt)
      .map((value) => value.checkedAt! + 30_000);
    if (!deadlines.length) return;
    const timer = window.setTimeout(
      () => {
        const time = Date.now();
        setExpansions((previous) =>
          Object.fromEntries(
            Object.entries(previous).map(([peer, value]) => [
              peer,
              value.page && time >= (value.checkedAt ?? 0) + 30_000
                ? { state: "loading", scope: value.scope }
                : value,
            ]),
          ),
        );
      },
      Math.max(0, Math.min(...deadlines) - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [expansions]);
  useEffect(() => {
    let live = true;
    const refresh = async () => {
      try {
        const peers = await graphPeers();
        if (!live) return;
        const ids = [...new Set(peers.map((peer) => peer.node_id))].sort();
        setPeerIds((previous) =>
          previous.length === ids.length &&
          previous.every((id, index) => id === ids[index])
            ? previous
            : ids,
        );
        setExpansions((previous) =>
          Object.fromEntries(
            Object.entries(previous).filter(([id]) => ids.includes(id)),
          ),
        );
      } catch {
        if (!live) return;
        setPeerIds([]);
        setExpansions({});
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 25_000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);
  const requestPeer = useCallback(
    async (peer: string, cursor: string | null, requestScope: string) => {
      if (document.hidden) return;
      const nonce = (requests.current.get(peer) ?? 0) + 1;
      requests.current.set(peer, nonce);
      setExpansions((previous) =>
        previous[peer]
          ? {
              ...previous,
              [peer]: {
                ...previous[peer],
                state: "loading",
                scope: requestScope,
                page:
                  previous[peer].scope === requestScope
                    ? previous[peer].page
                    : undefined,
              },
            }
          : previous,
      );
      try {
        let page: GraphPage;
        let windowCursor = cursor;
        const options = {
          node_id: peer,
          scope_workspace: graphWorkspace || null,
          depth: 1 as const,
          // Workspace relevance must not disappear when a perspective hides
          // Runs or the viewer turns off a display-only resource filter.
          mode: "mesh" as const,
          kinds: projectionKinds,
          relations: supportedRelations,
          hours,
          limit: 80,
          target_tenant: data.access.kind === "operator" ? targetTenant : null,
        };
        try {
          page = await graphPage({ ...options, cursor });
        } catch (error) {
          if (
            !(
              error instanceof Error &&
              "status" in error &&
              error.status === 409 &&
              cursor
            )
          )
            throw error;
          windowCursor = null;
          page = await graphPage({ ...options, cursor: null });
        }
        if (!validateGraphPage(page, peer))
          throw new Error("Invalid graph projection");
        if (requests.current.get(peer) !== nonce) return;
        setExpansions((previous) =>
          previous[peer]
            ? {
                ...previous,
                [peer]: {
                  state: page.nodes.length ? "ready" : "empty",
                  page,
                  windowCursor,
                  checkedAt: Date.now(),
                  scope: requestScope,
                },
              }
            : previous,
        );
      } catch (error) {
        if (requests.current.get(peer) !== nonce) return;
        setExpansions((previous) =>
          previous[peer]
            ? {
                ...previous,
                [peer]: { state: graphError(error), scope: requestScope },
              }
            : previous,
        );
      }
    },
    [graphWorkspace, hours, targetTenant, data.access.kind],
  );
  useEffect(() => {
    if (mode === "neighborhood" || document.hidden) return;
    for (const peer of Object.keys(expansionsRef.current)) {
      if (!peerIds.includes(peer)) continue;
      const current = expansionsRef.current[peer];
      void requestPeer(
        peer,
        current.scope === scope ? (current.windowCursor ?? null) : null,
        scope,
      );
    }
    // Only expansion membership, scope and refresh clock start a new request.
  }, [expandedKey, peerIds, scope, refreshTick, mode, requestPeer]);
  useEffect(() => {
    const onVisibility = () => {
      if (document.hidden) {
        for (const peer of Object.keys(expansionsRef.current))
          requests.current.set(peer, (requests.current.get(peer) ?? 0) + 1);
        setExpansions((previous) =>
          Object.fromEntries(
            Object.entries(previous).map(([peer, value]) => [
              peer,
              { state: "loading", scope: value.scope },
            ]),
          ),
        );
      } else setRefreshTick((value) => value + 1);
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [expandedKey]);
  useEffect(() => {
    const timer = window.setInterval(
      () => setRefreshTick((value) => value + 1),
      25_000,
    );
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (mode === "neighborhood") setExpansions({});
  }, [mode]);
  const pages = useMemo(
    () =>
      new Map(
        Object.entries(expansions)
          .filter(
            ([peer, value]) =>
              peerIds.includes(peer) &&
              value.page &&
              value.scope === scope &&
              now - (value.checkedAt ?? 0) < 30_000,
          )
          .map(([peer, value]) => [peer, value.page!] as const),
      ),
    [expansions, peerIds, scope, now],
  );
  const merged = useMemo(
    () =>
      mergeGraphPages(
        buildMeshGraph(data, {
          channel: graphWorkspace,
          runs,
          authorizedPeers: peerIds,
          hours,
          now,
        }),
        data.node.id,
        pages,
        runs,
      ),
    [data, graphWorkspace, runs, peerIds, pages, hours, now],
  );
  const full = useMemo(
    () => workspaceGraph(merged, data.node.id, graphWorkspace, data),
    [merged, data, graphWorkspace],
  );
  useEffect(() => {
    const stillVisible = (id: string) => {
      if (full.nodes.some((node) => node.id === id)) return true;
      const boundary = merged.nodes.find(
        (node) => node.id === id && node.kind === "remote",
      );
      return (
        !!boundary &&
        full.nodes.some(
          (node) =>
            node.nodeId === boundary.nodeId &&
            (node.kind === "agent" || node.kind === "run"),
        )
      );
    };
    if (selectedId && !stillVisible(selectedId)) setSelectedId("");
    if (focused && !stillVisible(focused)) setFocused("");
  }, [full, merged, selectedId, focused]);
  const graph = useMemo(() => {
    const boundary = merged.nodes.find(
      (node) => node.id === focused && node.kind === "remote",
    );
    return filterMeshGraph(full, {
      mode: graphMode,
      kinds,
      query: search,
      relations,
      focus: boundary
        ? full.nodes
            .filter(
              (node) =>
                node.nodeId === boundary.nodeId &&
                (node.kind === "agent" || node.kind === "run"),
            )
            .map((node) => node.id)
        : focused,
      pin: selectedId,
    });
  }, [full, merged, graphMode, kinds, search, relations, focused, selectedId]);
  const labels = useMemo(
    () =>
      disambiguateLabels(
        full.nodes,
        (n) => n.id,
        (n) => {
          const name = [
            n.name[locale],
            n.name[locale.slice(0, 2)],
            n.name.en,
            ...Object.values(n.name),
          ].find((v) => v?.trim());
          return name || copy.missing;
        },
        (n) => n.kind,
      ),
    [full.nodes, locale, copy],
  );
  const label = useCallback(
    (n: MeshNode) =>
      labels.get(n.id) ?? n.name[locale] ?? n.name.en ?? copy.missing,
    [labels, copy, locale],
  );
  const status = (n: MeshNode) =>
    n.status === "LOCAL"
      ? copy.localNode
      : n.status === "CONFIGURED"
        ? copy.configured
        : n.status === "DISCOVERED"
          ? copy.discovered
          : n.status
            ? t(n.status)
            : n.available
              ? copy.unknown
              : copy.missing;
  const selected =
    graph.nodes.find((n) => n.id === selectedId) ??
    merged.nodes.find(
      (n) =>
        n.id === selectedId && n.kind === "remote" && kinds.includes("remote"),
    ) ??
    (!selectedId
      ? (graph.nodes.find(
          (n) =>
            n.kind ===
              {
                mesh: "agent",
                collaboration: "workspace",
                knowledge: "artifact",
                execution: "task",
                topology: "cluster",
              }[graphMode] &&
            !n.remote &&
            n.available,
        ) ??
        graph.nodes.find((n) => n.kind === "workspace") ??
        graph.nodes[0])
      : undefined);
  const select = (id: string) => {
    setSelectedId(id);
    setClosed(false);
  };
  const togglePeer = (peer: string) => {
    if (expansions[peer]) {
      requests.current.set(peer, (requests.current.get(peer) ?? 0) + 1);
      setExpansions((previous) => {
        const next = { ...previous };
        delete next[peer];
        return next;
      });
      if (
        selectedId &&
        full.nodes.find((node) => node.id === selectedId)?.nodeId === peer
      )
        setSelectedId("");
    } else
      setExpansions((previous) => ({
        ...previous,
        [peer]: { state: "loading", scope },
      }));
  };
  const selectConnected = (id: string) => {
    if (!graph.nodes.some((n) => n.id === id)) {
      const target = merged.nodes.find((n) => n.id === id);
      setMode(target?.kind === "remote" ? "topology" : "mesh");
      setKinds([...meshKinds]);
      setSearch("");
      setFocused("");
      setRelations(undefined);
    }
    select(id);
  };
  const reset = () => {
    setKinds(
      graphMode === "mesh"
        ? meshKinds.filter(
            (kind) => !["model", "skill", "conversation"].includes(kind),
          )
        : [...meshKinds],
    );
    setSearch("");
    setRelations(undefined);
    setFocused("");
    setSelectedId("");
    if (focus) setFocus("");
  };
  const changeMode = (value: MeshMode | "neighborhood") => {
    if (value !== "neighborhood" && focus) setFocus("");
    setMode(value);
    setKinds(
      value === "mesh"
        ? meshKinds.filter(
            (kind) => !["model", "skill", "conversation"].includes(kind),
          )
        : [...meshKinds],
    );
    setFocused("");
    setSelectedId("");
    setClosed(false);
  };
  // The mesh workspace is context; conversations and model/skill controls live
  // in their dedicated perspectives so the legend leaves room for the tools.
  const shownKinds =
    graphMode === "mesh"
      ? modeKinds.mesh.filter(
          (kind) =>
            !["workspace", "conversation", "model", "skill"].includes(kind),
        )
      : modeKinds[graphMode];
  const relationTypes = [...new Set(full.edges.map((e) => e.relation))];
  const localEvents = data.events
    .filter(
      (e) =>
        (!graphWorkspace || e.workspace_id === graphWorkspace) &&
        inWindow(e.created_at, hours, now),
    )
    .map((event) => {
      const references = eventReferences(event);
      const target = full.nodes.find(
        (node) =>
          node.nodeId === data.node.id &&
          ((node.kind === "task" && node.resourceId === references.task_id) ||
            (node.kind === "artifact" &&
              node.resourceId === references.artifact_id)),
      );
      return {
        id: event.id,
        kind: event.kind,
        created_at: event.created_at,
        reference: target?.id ?? null,
      };
    });
  const events = mergeGraphTimeline(
    localEvents,
    pages,
    full.nodes,
    hours,
    Date.now(),
  );
  const tasks = data.tasks.filter(
    (task) => !graphWorkspace || task.workspace_id === graphWorkspace,
  );
  const timeline = events.slice(-80);
  const timelineStart = Date.parse(timeline[0]?.created_at ?? "");
  const timelineEnd = Date.parse(timeline.at(-1)?.created_at ?? "");
  const timelineTime = (value: number) =>
    new Date(value).toLocaleString(locale, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  const kindFilter = (kind: MeshKind) => (
    <label key={kind}>
      <span>
        <MeshIcon kind={kind} size={16} />
      </span>
      <span>{copy.kinds[kind]}</span>
      <input
        type="checkbox"
        checked={kinds.includes(kind)}
        onChange={(e) =>
          setKinds((values) =>
            e.target.checked
              ? [...values, kind]
              : values.filter((v) => v !== kind),
          )
        }
      />
    </label>
  );
  const inspector = !closed && selected;
  return (
    <section
      className={`mesh-graph mesh-${graphMode} ${inspector && mode !== "neighborhood" ? "has-inspector" : ""}`}
      aria-label="Graph View"
    >
      <div className="mesh-main">
        <header className="mesh-heading">
          <div>
            <h1>
              {mode === "knowledge" ||
              mode === "execution" ||
              mode === "topology"
                ? copy.modes[mode]
                : "Graph View"}
            </h1>
            <p>{copy.subtitle}</p>
          </div>
          <div className="mesh-toolbar">
            {mode !== "neighborhood" && (
              <label className="mesh-select">
                <span className="sr-only">{copy.workspaceScope}</span>
                <select
                  aria-label={copy.workspaceScope}
                  value={graphWorkspace}
                  onChange={(event) => {
                    setGraphWorkspace(event.target.value);
                    setSelectedId("");
                    setFocused("");
                  }}
                >
                  <option value="">{copy.allWorkspaces}</option>
                  {data.workspaces.map((workspace) => (
                    <option key={workspace.id} value={workspace.id}>
                      {workspace.title}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <label className="mesh-select">
              <Network size={14} />
              <span className="sr-only">{copy.mode}</span>
              <select
                aria-label={copy.mode}
                value={mode}
                onChange={(e) =>
                  changeMode(e.target.value as MeshMode | "neighborhood")
                }
              >
                {(Object.keys(copy.modes) as MeshMode[]).map((v) => (
                  <option key={v} value={v}>
                    {copy.modes[v]}
                  </option>
                ))}
                <option value="neighborhood">{copy.fullDetails}</option>
              </select>
            </label>
            {mode !== "neighborhood" && (
              <label className="mesh-select" title={copy.windowHelp}>
                <Clock3 size={14} />
                <span className="sr-only">{copy.time}</span>
                <select
                  aria-label={copy.time}
                  value={hours}
                  onChange={(e) => setHours(Number(e.target.value))}
                >
                  <option value={1}>{copy.hour}</option>
                  <option value={24}>{copy.day}</option>
                  <option value={168}>{copy.week}</option>
                  <option value={720}>{copy.month}</option>
                  <option value={0}>{copy.allTime}</option>
                </select>
              </label>
            )}
            {mode !== "neighborhood" && (
              <label className="mesh-select">
                <span className="sr-only">{copy.layout}</span>
                <select
                  aria-label={copy.layout}
                  value={layout}
                  onChange={(e) => setLayout(e.target.value as MeshLayout)}
                >
                  <option value="structured">{copy.mesh}</option>
                  <option value="force">{copy.force}</option>
                  <option value="circle">{copy.radial}</option>
                </select>
              </label>
            )}
          </div>
        </header>
        {mode === "neighborhood" ? (
          <div className="mesh-neighborhood">
            <Suspense fallback={<p role="status">…</p>}>
              <Neighborhood
                data={data}
                channel={channel}
                focus={focus}
                setFocus={setFocus}
                visitChannel={visitChannel}
                open={open}
                discovery={discovery}
                runs={runs}
              />
            </Suspense>
          </div>
        ) : (
          <>
            <div className="mesh-view-controls">
              <Button
                variant="outline"
                type="button"
                className="mesh-filter-toggle"
                aria-expanded={filtersOpen}
                onClick={() => setFiltersOpen((v) => !v)}
              >
                <Filter size={14} />
                {copy.filters}
              </Button>
              <span>
                {graph.nodes.length} {copy.counts}
                <span className="mesh-counter-divider">/</span>
                {graph.edges.length} {copy.links}
              </span>
              <div className="mesh-view-actions">
                <Button
                  variant="outline"
                  type="button"
                  title={focused ? copy.unfocus : copy.focus}
                  aria-label={focused ? copy.unfocus : copy.focus}
                  aria-pressed={Boolean(focused)}
                  disabled={!selected && !focused}
                  onClick={() =>
                    setFocused(focused ? "" : (selected?.id ?? ""))
                  }
                >
                  <Focus size={15} />
                </Button>
                <Button
                  variant="outline"
                  type="button"
                  title={list ? copy.canvas : copy.list}
                  aria-label={list ? copy.canvas : copy.list}
                  aria-pressed={list}
                  onClick={() => setList((v) => !v)}
                >
                  <List size={15} />
                </Button>
                <Button
                  variant="outline"
                  type="button"
                  aria-label={copy.clear}
                  title={copy.clear}
                  onClick={reset}
                >
                  <RotateCcw size={14} />
                </Button>
              </div>
            </div>
            {peerIds.length > 0 && (
              <div
                className="mesh-peer-controls"
                aria-label={copy.kinds.remote}
              >
                {data.access.kind === "operator" && (
                  <label>
                    {copy.peerScope}
                    <input
                      aria-label={copy.peerScope}
                      aria-invalid={
                        targetTenant.length > 0 &&
                        !validGraphTenant(targetTenant)
                      }
                      value={targetTenant}
                      onChange={(event) => {
                        setTargetTenant(event.target.value);
                        setExpansions({});
                        setSelectedId("");
                      }}
                    />
                    {targetTenant.length > 0 &&
                      !validGraphTenant(targetTenant) && (
                        <span role="alert">{copy.peerInvalidTenant}</span>
                      )}
                  </label>
                )}
                {peerIds.map((peer) => {
                  const expansion = expansions[peer];
                  const state = expansion?.state;
                  const stateLabel =
                    state === "loading"
                      ? copy.peerLoading
                      : state === "empty"
                        ? expansion?.page?.next_cursor
                          ? copy.peerEmptyPage
                          : copy.peerEmpty
                        : state === "denied"
                          ? copy.peerDenied
                          : state === "unsupported"
                            ? copy.peerUnsupported
                            : state === "oversized"
                              ? copy.peerOversized
                              : state === "invalid"
                                ? copy.peerInvalidTenant
                                : state === "unavailable"
                                  ? copy.peerUnavailable
                                  : "";
                  return (
                    <div className="mesh-peer" key={peer}>
                      <span>{peer}</span>
                      <Button
                        variant="outline"
                        type="button"
                        onClick={() => togglePeer(peer)}
                        disabled={
                          data.access.kind === "operator" &&
                          !validGraphTenant(targetTenant)
                        }
                      >
                        {expansion ? copy.collapsePeer : copy.expandPeer}
                      </Button>
                      {expansion && (
                        <Button
                          variant="outline"
                          type="button"
                          onClick={() => void requestPeer(peer, null, scope)}
                        >
                          {copy.refreshPeer}
                        </Button>
                      )}
                      {expansion?.page?.next_cursor &&
                        expansion.scope === scope && (
                          <Button
                            variant="outline"
                            type="button"
                            onClick={() =>
                              void requestPeer(
                                peer,
                                expansion.page!.next_cursor,
                                scope,
                              )
                            }
                          >
                            {copy.loadMorePeer}
                          </Button>
                        )}
                      {stateLabel && <small role="status">{stateLabel}</small>}
                    </div>
                  );
                })}
              </div>
            )}
            {mode === "execution" && (
              <div className="mesh-statistics">
                {(
                  [
                    [
                      copy.running,
                      tasks.filter((v) =>
                        ["RUNNING", "CLAIMED"].includes(v.status),
                      ).length,
                      "active",
                    ],
                    [
                      copy.blocked,
                      tasks.filter((v) =>
                        ["BLOCKED", "FAILED"].includes(v.status),
                      ).length,
                      "blocked",
                    ],
                    [
                      copy.pending,
                      tasks.filter((v) => v.status === "OPEN").length,
                      "pending",
                    ],
                    [
                      copy.completed,
                      tasks.filter((v) => v.status === "COMPLETED").length,
                      "done",
                    ],
                  ] as const
                ).map(([name, value, state]) => (
                  <div key={state} className={state}>
                    <span>{name}</span>
                    <strong>{value}</strong>
                  </div>
                ))}
              </div>
            )}
            <div className="mesh-stage">
              <aside
                className={`mesh-filters ${filtersOpen ? "is-open" : ""}`}
                aria-label={copy.filters}
              >
                <fieldset>
                  <legend>{copy.types}</legend>
                  {shownKinds
                    .filter((kind) => !["model", "skill"].includes(kind))
                    .map(kindFilter)}
                </fieldset>
                {shownKinds.some((kind) =>
                  ["model", "skill"].includes(kind),
                ) && (
                  <details>
                    <summary>{copy.advancedTypes}</summary>
                    {shownKinds
                      .filter((kind) => ["model", "skill"].includes(kind))
                      .map(kindFilter)}
                  </details>
                )}
                <details>
                  <summary>{copy.relationsLabel}</summary>
                  <fieldset>
                    <legend className="sr-only">{copy.relationsLabel}</legend>
                    {relationTypes.map((relation) => (
                      <label key={relation}>
                        <span>{copy.relations[relation]}</span>
                        <input
                          type="checkbox"
                          checked={!relations || relations.includes(relation)}
                          onChange={(e) =>
                            setRelations((values) =>
                              e.target.checked
                                ? [...(values ?? relationTypes), relation]
                                : (values ?? relationTypes).filter(
                                    (v) => v !== relation,
                                  ),
                            )
                          }
                        />
                      </label>
                    ))}
                  </fieldset>
                </details>
              </aside>
              {graph.nodes.length === 0 && (
                <div className="mesh-empty" role="status">
                  <Network size={36} />
                  <h2>{copy.noResults}</h2>
                  <p>{full.nodes.length ? copy.snapshot : copy.empty}</p>
                  <Button variant="outline" type="button" onClick={reset}>
                    {copy.clear}
                  </Button>
                </div>
              )}
              {list ? (
                <div className="mesh-list">
                  <table aria-label={copy.list}>
                    <thead>
                      <tr>
                        <th>{copy.node}</th>
                        <th>{copy.type}</th>
                        <th>{copy.relationsLabel}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {graph.nodes.map((n) => (
                        <tr key={n.id}>
                          <th scope="row">
                            <Button
                              variant="outline"
                              type="button"
                              onClick={() => select(n.id)}
                            >
                              <MeshIcon kind={n.kind} size={16} />
                              {label(n)}
                            </Button>
                          </th>
                          <td>{copy.kinds[n.kind]}</td>
                          <td>
                            {graph.edges
                              .filter(
                                (e) => e.source === n.id || e.target === n.id,
                              )
                              .map((e) => (
                                <div key={e.id}>
                                  {label(
                                    graph.nodes.find((v) => v.id === e.source)!,
                                  )}{" "}
                                  → {copy.relations[e.relation]} →{" "}
                                  {label(
                                    graph.nodes.find((v) => v.id === e.target)!,
                                  )}
                                </div>
                              ))}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : (
                <Suspense fallback={<p role="status">…</p>}>
                  <MeshCanvas
                    graph={graph}
                    mode={graphMode}
                    layout={layout}
                    selectedId={selected?.id ?? ""}
                    select={select}
                    label={label}
                    status={status}
                    copy={copy}
                    localNode={data.node.id}
                    showNodeFrames={kinds.includes("remote")}
                  />
                </Suspense>
              )}
            </div>
            {mode === "execution" && (
              <div className="mesh-timeline" aria-label={copy.timeline}>
                <strong>
                  {copy.timeline}
                  <span>
                    {events.length} {copy.events}
                  </span>
                </strong>
                {events.length ? (
                  <div className="mesh-timeline-track">
                    {timeline.map((event, index) => {
                      const target = full.nodes.find(
                        (node) => node.id === event.reference,
                      );
                      const progress =
                        timelineEnd === timelineStart
                          ? 0.5
                          : (Date.parse(event.created_at) - timelineStart) /
                            (timelineEnd - timelineStart);
                      return (
                        <Button
                          variant="outline"
                          type="button"
                          key={event.id}
                          disabled={!target}
                          style={{
                            left: `${2 + progress * 96}%`,
                            top: index % 2 ? 16 : 2,
                          }}
                          title={`${event.kind} · ${new Date(event.created_at).toLocaleString(locale)}`}
                          aria-label={`${event.kind} · ${new Date(event.created_at).toLocaleString(locale)}`}
                          onClick={() => target && selectConnected(target.id)}
                        >
                          <span
                            className={
                              /fail|block/i.test(event.kind) ? "warning" : ""
                            }
                          />
                        </Button>
                      );
                    })}
                  </div>
                ) : (
                  <p>{copy.noTimeline}</p>
                )}
                {timeline.length > 0 && (
                  <div className="mesh-timeline-times">
                    <time dateTime={timeline[0].created_at}>
                      {timelineTime(timelineStart)}
                    </time>
                    <time dateTime={timeline.at(-1)!.created_at}>
                      {timelineTime(timelineEnd)}
                    </time>
                  </div>
                )}
                {events.length > timeline.length && (
                  <p>
                    {copy.omitted}: {events.length - timeline.length}{" "}
                    {copy.events}
                  </p>
                )}
              </div>
            )}
            <footer className="mesh-footnote">
              <span title={copy.live}>
                <i />
                {copy.snapshot}
              </span>
              {(graph.omitted > 0 || graph.omittedEdges > 0) && (
                <span role="status">
                  {copy.omitted}: {graph.omitted} {copy.counts} ·{" "}
                  {graph.omittedEdges} {copy.links}
                </span>
              )}
            </footer>
          </>
        )}
      </div>
      {mode !== "neighborhood" && inspector && (
        <MeshInspector
          key={selected.id}
          node={selected}
          graph={full}
          data={data}
          hours={hours}
          now={now}
          channel={graphWorkspace}
          copy={copy}
          label={label}
          status={status}
          select={selectConnected}
          close={() => setClosed(true)}
          open={open}
          visitChannel={visitChannel}
          remoteActivity={pages
            .get(selected.nodeId)
            ?.activity.filter((marker) => marker.reference === selected.id)}
        />
      )}
    </section>
  );
}
