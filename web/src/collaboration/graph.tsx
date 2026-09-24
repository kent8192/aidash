import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import { Clock3, Filter, Focus, List, Network, RotateCcw } from "lucide-react";
import type { Discovery, Run, State } from "../types";
import { useI18n } from "../ui";
import { disambiguateLabels } from "../display-labels";
import { meshCopy } from "./mesh-copy";
import { meshColors, MeshIcon } from "./mesh-icons";
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
import { MeshInspector } from "./mesh-inspector";
import type { MeshLayout } from "./mesh-canvas";
import type { Selection } from "./details";

const MeshCanvas = lazy(() =>
  import("./mesh-canvas").then((m) => ({ default: m.MeshCanvas })),
);
const Neighborhood = lazy(() =>
  import("./agent-neighborhood").then((m) => ({ default: m.Graph })),
);

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
  const [mode, setMode] = useState<MeshMode | "neighborhood">(
    focus ? "neighborhood" : "mesh",
  );
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
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const full = useMemo(
    () => buildMeshGraph(data, { channel, discovery, runs, hours, now }),
    [data, channel, discovery, runs, hours, now],
  );
  const graphMode = mode === "neighborhood" ? "mesh" : mode;
  const graph = useMemo(
    () =>
      filterMeshGraph(full, {
        mode: graphMode,
        kinds,
        query: search,
        relations,
        focus: focused,
      }),
    [full, graphMode, kinds, search, relations, focused],
  );
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
    (n: MeshNode) => labels.get(n.id) ?? copy.missing,
    [labels, copy],
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
  const selectConnected = (id: string) => {
    if (!graph.nodes.some((n) => n.id === id)) {
      const target = full.nodes.find((n) => n.id === id);
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
  const events = data.events
    .filter(
      (e) =>
        (!channel || e.workspace_id === channel) &&
        inWindow(e.created_at, hours, now),
    )
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
  const tasks = data.tasks.filter(
    (task) => !channel || task.workspace_id === channel,
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
      <span style={{ color: meshColors[kind] }}>
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
                {(Object.keys(copy.modes) as MeshMode[])
                  .filter(
                    (v) => v !== "topology" || data.access.kind === "operator",
                  )
                  .map((v) => (
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
              <button
                type="button"
                className="mesh-filter-toggle"
                aria-expanded={filtersOpen}
                onClick={() => setFiltersOpen((v) => !v)}
              >
                <Filter size={14} />
                {copy.filters}
              </button>
              <span>
                {graph.nodes.length} {copy.counts}
                <span className="mesh-counter-divider">/</span>
                {graph.edges.length} {copy.links}
              </span>
              <div className="mesh-view-actions">
                <button
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
                </button>
                <button
                  type="button"
                  title={list ? copy.canvas : copy.list}
                  aria-label={list ? copy.canvas : copy.list}
                  aria-pressed={list}
                  onClick={() => setList((v) => !v)}
                >
                  <List size={15} />
                </button>
                <button
                  type="button"
                  aria-label={copy.clear}
                  title={copy.clear}
                  onClick={reset}
                >
                  <RotateCcw size={14} />
                </button>
              </div>
            </div>
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
              {graph.nodes.length === 0 ? (
                <div className="mesh-empty" role="status">
                  <Network size={36} />
                  <h2>{copy.noResults}</h2>
                  <p>{full.nodes.length ? copy.snapshot : copy.empty}</p>
                  <button type="button" onClick={reset}>
                    {copy.clear}
                  </button>
                </div>
              ) : list ? (
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
                            <button type="button" onClick={() => select(n.id)}>
                              <MeshIcon kind={n.kind} size={16} />
                              {label(n)}
                            </button>
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
                      const d = eventReferences(event);
                      const target = full.nodes.find(
                        (v) =>
                          (v.kind === "task" && v.resourceId === d.task_id) ||
                          (v.kind === "artifact" &&
                            v.resourceId === d.artifact_id),
                      );
                      const progress =
                        timelineEnd === timelineStart
                          ? 0.5
                          : (Date.parse(event.created_at) - timelineStart) /
                            (timelineEnd - timelineStart);
                      return (
                        <button
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
                        </button>
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
          channel={channel}
          copy={copy}
          label={label}
          status={status}
          select={selectConnected}
          close={() => setClosed(true)}
          open={open}
          visitChannel={visitChannel}
        />
      )}
    </section>
  );
}
