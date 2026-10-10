import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Kbd } from "../components/ui/kbd";
import { NativeSelect } from "../components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../components/ui/table";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Loading } from "../components/patterns";
import {
  Focus,
  List,
  Network,
  RotateCcw,
  Search,
  SlidersHorizontal,
} from "lucide-react";
import type { Discovery, Run, State } from "../types";
import { cn } from "../lib/utils";
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
import { floatingToolbar, MeshIcon } from "./mesh-icons";
import { ExecutionLanes } from "./graph-lanes";
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
}: {
  data: State;
  channel: string;
  focus: string;
  discovery?: Discovery;
  runs: { run: Run; node: string }[];
  setFocus: (id: string) => void;
  visitChannel: (id: string) => void;
  open: (selection: Selection) => void;
}) {
  const { locale, t } = useI18n();
  const copy = meshCopy[locale];
  const [search, setSearch] = useState("");
  const searchField = useRef<HTMLInputElement>(null);
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
        run: references.run_id,
        task: references.task_id,
      };
    });
  const events = mergeGraphTimeline(
    localEvents,
    pages,
    full.nodes,
    hours,
    Date.now(),
  );
  const localLanes = new Map(localEvents.map((event) => [event.id, event]));
  const laneEvents = events.map((event) => ({
    ...event,
    run: localLanes.get(event.id)?.run,
    task: localLanes.get(event.id)?.task,
  }));
  const tasks = data.tasks.filter(
    (task) => !graphWorkspace || task.workspace_id === graphWorkspace,
  );
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        event.key !== "/" ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        (event.target instanceof Element &&
          event.target.closest(
            "input, textarea, select, [contenteditable='true']",
          ))
      )
        return;
      const field = searchField.current;
      if (!field) return;
      event.preventDefault();
      setFiltersOpen(true);
      requestAnimationFrame(() => field.focus());
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const toggleRow = (
    key: string,
    name: string,
    count: number,
    checked: boolean,
    change: (checked: boolean) => void,
    glyph: ReactNode,
  ) => (
    <label
      key={key}
      className="flex h-7 cursor-pointer items-center gap-2 rounded-md px-1.5 text-[13px] transition-colors hover:bg-accent"
    >
      <span className="grid w-4 shrink-0 place-items-center text-faint">
        {glyph}
      </span>
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <span
        aria-hidden="true"
        className="font-mono text-[11px] text-faint tabular"
      >
        {count}
      </span>
      <input
        type="checkbox"
        aria-label={name}
        checked={checked}
        onChange={(event) => change(event.target.checked)}
        className="size-3.5 shrink-0 cursor-pointer appearance-none rounded-sm border border-border-strong bg-surface transition-colors checked:border-primary checked:bg-primary checked:shadow-[inset_0_0_0_2px_var(--surface)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      />
    </label>
  );
  const kindFilter = (kind: MeshKind) =>
    toggleRow(
      kind,
      copy.kinds[kind],
      full.nodes.filter((node) => node.kind === kind).length,
      kinds.includes(kind),
      (checked) =>
        setKinds((values) =>
          checked ? [...values, kind] : values.filter((v) => v !== kind),
        ),
      <MeshIcon kind={kind} size={14} />,
    );
  const relationGlyph = (relation: MeshRelation) =>
    relation === "depends"
      ? "border-dashed border-warning"
      : relation === "model"
        ? "border-dotted border-edge-strong"
        : relation === "federation"
          ? "border-dashed border-edge"
          : [
                "delegates",
                "coordinates",
                "assigned",
                "executes",
                "creates",
              ].includes(relation)
            ? "border-edge-strong"
            : "border-edge";
  const group = (title: string, children: ReactNode) => (
    <div className="grid gap-0.5 border-b border-border px-2 py-2.5 last:border-b-0">
      <span className="px-1.5 pb-1 text-[11px] font-medium text-faint">
        {title}
      </span>
      {children}
    </div>
  );
  const statistics = [
    [
      copy.running,
      tasks.filter((v) => ["RUNNING", "CLAIMED"].includes(v.status)).length,
      "text-brand",
    ],
    [
      copy.blocked,
      tasks.filter((v) => ["BLOCKED", "FAILED"].includes(v.status)).length,
      "text-warning",
    ],
    [
      copy.pending,
      tasks.filter((v) => v.status === "OPEN").length,
      "text-foreground",
    ],
    [
      copy.completed,
      tasks.filter((v) => v.status === "COMPLETED").length,
      "text-success",
    ],
  ] as const;
  const perspective = (
    <NativeSelect
      aria-label={copy.mode}
      value={mode}
      onChange={(e) => changeMode(e.target.value as MeshMode | "neighborhood")}
    >
      {(Object.keys(copy.modes) as MeshMode[]).map((v) => (
        <option key={v} value={v}>
          {copy.modes[v]}
        </option>
      ))}
      <option value="neighborhood">{copy.fullDetails}</option>
    </NativeSelect>
  );
  const viewActions = (
    <>
      <Button
        variant="ghost"
        size="icon"
        className="size-7"
        title={focused ? copy.unfocus : copy.focus}
        aria-label={focused ? copy.unfocus : copy.focus}
        aria-pressed={Boolean(focused)}
        disabled={!selected && !focused}
        onClick={() => setFocused(focused ? "" : (selected?.id ?? ""))}
      >
        <Focus />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-7"
        title={list ? copy.canvas : copy.list}
        aria-label={list ? copy.canvas : copy.list}
        aria-pressed={list}
        onClick={() => setList((v) => !v)}
      >
        {list ? <Network /> : <List />}
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-7"
        aria-label={copy.clear}
        title={copy.clear}
        onClick={reset}
      >
        <RotateCcw />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-7 lg:hidden"
        aria-label={copy.filters}
        title={copy.filters}
        aria-expanded={filtersOpen}
        onClick={() => setFiltersOpen((v) => !v)}
      >
        <SlidersHorizontal />
      </Button>
    </>
  );
  const inspector = !closed && selected;
  return (
    <section
      className={cn(
        "mesh-graph flex h-full min-h-0 flex-1 flex-col bg-background",
        `mesh-${graphMode}`,
      )}
      aria-label="Graph View"
    >
      {mode === "neighborhood" ? (
        <>
          <div className="flex h-11 shrink-0 items-center gap-3 border-b border-border px-4">
            <span className="text-[11px] font-medium text-faint">
              {copy.mode}
            </span>
            <span className="w-56 max-w-full">{perspective}</span>
          </div>
          <div className="min-h-0 flex-1">
            <Suspense fallback={<Loading className="p-4">…</Loading>}>
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
        </>
      ) : (
        <>
          <div className="relative flex min-h-0 flex-1">
            <div className="mesh-stage canvas-grid relative min-h-0 min-w-0 flex-1 overflow-hidden">
              {list ? (
                <>
                  <div className="mesh-list absolute inset-3 top-14 z-10 overflow-auto rounded-lg border border-border bg-surface lg:left-[272px]">
                    <Table aria-label={copy.list}>
                      <TableHeader>
                        <TableRow>
                          <TableHead>{copy.node}</TableHead>
                          <TableHead>{copy.type}</TableHead>
                          <TableHead>{copy.relationsLabel}</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {graph.nodes.map((n) => (
                          <TableRow
                            key={n.id}
                            data-state={
                              selected?.id === n.id ? "selected" : undefined
                            }
                          >
                            <th
                              scope="row"
                              className="w-[34%] px-1.5 py-1 text-left align-top font-normal"
                            >
                              <button
                                type="button"
                                className="flex min-h-7 w-full items-center gap-2 rounded-md px-1.5 text-left transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
                                onClick={() => select(n.id)}
                              >
                                <MeshIcon
                                  kind={n.kind}
                                  size={14}
                                  className="shrink-0 text-muted-foreground"
                                />
                                {label(n)}
                              </button>
                            </th>
                            <TableCell className="pb-2 pt-[11px] align-top text-xs text-faint">
                              {copy.kinds[n.kind]}
                            </TableCell>
                            <TableCell className="pb-2 pt-[11px] align-top text-xs leading-5 text-muted-foreground">
                              {graph.edges
                                .filter(
                                  (e) => e.source === n.id || e.target === n.id,
                                )
                                .map((e) => (
                                  <div key={e.id}>
                                    {label(
                                      graph.nodes.find(
                                        (v) => v.id === e.source,
                                      )!,
                                    )}{" "}
                                    → {copy.relations[e.relation]} →{" "}
                                    {label(
                                      graph.nodes.find(
                                        (v) => v.id === e.target,
                                      )!,
                                    )}
                                  </div>
                                ))}
                            </TableCell>
                          </TableRow>
                        ))}
                      </TableBody>
                    </Table>
                  </div>
                  <div
                    className={floatingToolbar}
                    role="group"
                    aria-label={copy.camera}
                  >
                    {viewActions}
                  </div>
                </>
              ) : (
                <Suspense fallback={<Loading className="p-4">…</Loading>}>
                  <MeshCanvas
                    graph={graph}
                    mode={graphMode}
                    layout={layout}
                    selectedId={selected?.id ?? ""}
                    focusId={selectedId}
                    select={select}
                    label={label}
                    status={status}
                    copy={copy}
                    localNode={data.node.id}
                    showNodeFrames={kinds.includes("remote")}
                    actions={viewActions}
                  />
                </Suspense>
              )}
              {graph.nodes.length === 0 && (
                <div
                  className="mesh-empty pointer-events-none absolute inset-0 z-10 grid place-items-center p-6 lg:pl-[272px]"
                  role="status"
                >
                  <div className="pointer-events-auto grid max-w-sm justify-items-center gap-2 rounded-lg border border-border bg-surface px-6 py-5 text-center shadow-overlay">
                    <Network size={22} className="text-faint" aria-hidden />
                    <h2 className="text-[15px] font-semibold">
                      {copy.noResults}
                    </h2>
                    <p className="text-xs text-muted-foreground">
                      {full.nodes.length ? copy.snapshot : copy.empty}
                    </p>
                    <Button variant="outline" size="sm" onClick={reset}>
                      {copy.clear}
                    </Button>
                  </div>
                </div>
              )}
              <aside
                className={cn(
                  "mesh-filters absolute bottom-3 left-3 top-3 z-20 flex w-[248px] flex-col overflow-hidden rounded-lg border border-border-strong bg-surface/95 shadow-overlay max-lg:top-14 max-lg:z-40 max-lg:w-[min(280px,calc(100%-24px))]",
                  !filtersOpen && "max-lg:hidden",
                )}
                aria-label={copy.filters}
              >
                <div className="min-h-0 flex-1 overflow-y-auto">
                  <div className="grid gap-2 border-b border-border p-3">
                    <label className="relative block">
                      <Search
                        aria-hidden
                        className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
                      />
                      <Input
                        ref={searchField}
                        type="search"
                        aria-label={copy.search}
                        placeholder={copy.search}
                        value={search}
                        onChange={(event) => setSearch(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Escape")
                            event.currentTarget.blur();
                        }}
                        className="h-8 pl-8 pr-8 [&::-webkit-search-cancel-button]:hidden"
                      />
                      <Kbd className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2">
                        /
                      </Kbd>
                    </label>
                    {perspective}
                    <NativeSelect
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
                    </NativeSelect>
                    <div className="grid grid-cols-2 gap-2">
                      <NativeSelect
                        aria-label={copy.time}
                        title={copy.windowHelp}
                        value={hours}
                        onChange={(e) => setHours(Number(e.target.value))}
                      >
                        <option value={1}>{copy.hour}</option>
                        <option value={24}>{copy.day}</option>
                        <option value={168}>{copy.week}</option>
                        <option value={720}>{copy.month}</option>
                        <option value={0}>{copy.allTime}</option>
                      </NativeSelect>
                      <NativeSelect
                        aria-label={copy.layout}
                        value={layout}
                        onChange={(e) =>
                          setLayout(e.target.value as MeshLayout)
                        }
                      >
                        <option value="structured">{copy.mesh}</option>
                        <option value="force">{copy.force}</option>
                        <option value="circle">{copy.radial}</option>
                      </NativeSelect>
                    </div>
                  </div>
                  {group(
                    copy.types,
                    shownKinds
                      .filter((kind) => !["model", "skill"].includes(kind))
                      .map(kindFilter),
                  )}
                  {shownKinds.some((kind) =>
                    ["model", "skill"].includes(kind),
                  ) &&
                    group(
                      copy.advancedTypes,
                      shownKinds
                        .filter((kind) => ["model", "skill"].includes(kind))
                        .map(kindFilter),
                    )}
                  {group(
                    copy.relationsLabel,
                    relationTypes.map((relation) =>
                      toggleRow(
                        relation,
                        copy.relations[relation],
                        full.edges.filter((e) => e.relation === relation)
                          .length,
                        !relations || relations.includes(relation),
                        (checked) =>
                          setRelations((values) =>
                            checked
                              ? [...(values ?? relationTypes), relation]
                              : (values ?? relationTypes).filter(
                                  (v) => v !== relation,
                                ),
                          ),
                        <i
                          className={cn(
                            "block w-4 border-t",
                            relationGlyph(relation),
                          )}
                        />,
                      ),
                    ),
                  )}
                  {peerIds.length > 0 &&
                    group(
                      copy.peers,
                      <div
                        className="mesh-peer-controls grid gap-2.5 px-1.5"
                        role="group"
                        aria-label={copy.peers}
                      >
                        {data.access.kind === "operator" && (
                          <label className="grid gap-1 text-[11px] text-faint">
                            {copy.peerScope}
                            <Input
                              aria-label={copy.peerScope}
                              aria-invalid={
                                targetTenant.length > 0 &&
                                !validGraphTenant(targetTenant)
                              }
                              value={targetTenant}
                              className="h-7 font-mono text-xs"
                              onChange={(event) => {
                                setTargetTenant(event.target.value);
                                setExpansions({});
                                setSelectedId("");
                              }}
                            />
                            {targetTenant.length > 0 &&
                              !validGraphTenant(targetTenant) && (
                                <span role="alert" className="text-destructive">
                                  {copy.peerInvalidTenant}
                                </span>
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
                            <div className="mesh-peer grid gap-1.5" key={peer}>
                              <span className="truncate font-mono text-xs text-muted-foreground">
                                {peer}
                              </span>
                              <div className="flex flex-wrap gap-1.5">
                                <Button
                                  variant={expansion ? "secondary" : "outline"}
                                  size="sm"
                                  type="button"
                                  onClick={() => togglePeer(peer)}
                                  disabled={
                                    data.access.kind === "operator" &&
                                    !validGraphTenant(targetTenant)
                                  }
                                >
                                  {expansion
                                    ? copy.collapsePeer
                                    : copy.expandPeer}
                                </Button>
                                {expansion && (
                                  <Button
                                    variant="outline"
                                    size="sm"
                                    type="button"
                                    onClick={() =>
                                      void requestPeer(peer, null, scope)
                                    }
                                  >
                                    {copy.refreshPeer}
                                  </Button>
                                )}
                                {expansion?.page?.next_cursor &&
                                  expansion.scope === scope && (
                                    <Button
                                      variant="outline"
                                      size="sm"
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
                              </div>
                              {stateLabel && (
                                <small
                                  role="status"
                                  className="text-[11px] text-faint"
                                >
                                  {stateLabel}
                                </small>
                              )}
                            </div>
                          );
                        })}
                      </div>,
                    )}
                  {group(
                    copy.status,
                    <div className="mesh-statistics grid grid-cols-2 gap-2 px-1.5">
                      {statistics.map(([name, value, tone]) => (
                        <div key={name} className="grid">
                          <strong
                            className={cn(
                              "font-mono text-[15px] font-medium leading-tight tabular",
                              tone,
                            )}
                          >
                            {value}
                          </strong>
                          <span className="text-[11px] text-faint">{name}</span>
                        </div>
                      ))}
                    </div>,
                  )}
                </div>
                <footer className="mesh-footnote grid shrink-0 gap-0.5 border-t border-border px-3 py-2 text-[11px] text-faint">
                  <span className="font-mono tabular" title={copy.live}>
                    {graph.nodes.length} {copy.counts} · {graph.edges.length}{" "}
                    {copy.links}
                  </span>
                  {(graph.omitted > 0 || graph.omittedEdges > 0) && (
                    <span role="status">
                      {copy.omitted}: {graph.omitted} {copy.counts} ·{" "}
                      {graph.omittedEdges} {copy.links}
                    </span>
                  )}
                </footer>
              </aside>
            </div>
            {inspector && (
              <MeshInspector
                key={selected.id}
                node={selected}
                graph={full}
                data={data}
                runs={runs}
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
                  ?.activity.filter(
                    (marker) => marker.reference === selected.id,
                  )}
              />
            )}
          </div>
          {mode === "execution" && (
            <ExecutionLanes
              tasks={tasks}
              runs={runs.filter(
                ({ run }) =>
                  !graphWorkspace || run.workspace_id === graphWorkspace,
              )}
              events={laneEvents}
              nodes={full.nodes}
              label={label}
              select={selectConnected}
              copy={copy}
              locale={locale}
              hours={hours}
              now={now}
            />
          )}
        </>
      )}
    </section>
  );
}
