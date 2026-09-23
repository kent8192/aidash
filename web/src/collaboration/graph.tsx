import { ReferenceName } from "../record-view";
import { lazy, Suspense, useState } from "react";
import type { State, Discovery, Run } from "../types";
import { Badge, useI18n, useAgentLabel } from "../ui";
import {
  buildAgentGraph,
  entityKey,
  graphKinds,
  type GraphKind,
  type GraphNode,
} from "../agent-graph/model";
const CytoscapeCanvas = lazy(() =>
  import("./cytoscape-canvas").then((module) => ({
    default: module.CytoscapeCanvas,
  })),
);
import { graphCopy } from "../agent-graph/copy";
import { relatedChannels } from "./model";
import { collaborationCopy } from "./copy";
import { MeshView } from "./topology";
import type { Selection } from "./details";
import "../agent-graph/style.css";

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
  const { local, locale } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale];
  const graphText = graphCopy[locale];
  const [mode, setMode] = useState<"relationships" | "topology">(
    "relationships",
  );
  const [view, setView] = useState<"graph" | "list">("graph");
  const [expanded, setExpanded] = useState<string[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [kinds, setKinds] = useState<GraphKind[]>([...graphKinds]);
  const [runtime, setRuntime] = useState(true);
  const agents = data.registry.filter((entry) => entry.kind === "agent");
  const channelAgents = new Set(
    data.runs
      .filter(
        (run) => run.workspace_id === channel && run.home_node === data.node.id,
      )
      .map((run) =>
        entityKey(data.node.id, "agent", {
          id: run.agent_id,
          version: run.agent_version,
        }),
      ),
  );
  const root = focus
    ? agents.find((entry) => entityKey(data.node.id, "agent", entry) === focus)
    : (agents.find((entry) =>
        channelAgents.has(entityKey(data.node.id, "agent", entry)),
      ) ?? agents[0]);
  const graph = root
    ? buildAgentGraph(
        {
          nodeId: data.node.id,
          root: { id: root.id, version: root.version },
          entries: data.registry,
          runs: data.runs,
          tasks: data.tasks,
        },
        { expandedIds: expanded, kinds, runtime },
      )
    : null;
  const selected =
    graph?.nodes.find((node) => node.id === selectedId) ?? graph?.nodes[0];
  const related = selected
    ? relatedChannels(
        selected,
        {
          nodeId: data.node.id,
          workspaces: data.workspaces,
          tasks: data.tasks,
          runs: data.runs,
          registry: data.registry,
        },
        channel,
      )
    : [];
  const label = (node: GraphNode) => {
    const name = local(node.name) || graphText.types[node.kind];
    return node.entity ? `${name} · v${node.entity.version}` : name;
  };
  const inspect = (node: GraphNode) => {
    if (!node.available) return;
    if (node.entity) {
      const entity = data.registry.find(
        (entry) =>
          entry.kind === node.kind &&
          entry.id === node.entity!.id &&
          entry.version === node.entity!.version,
      );
      if (entity) open({ kind: "entityDetail", entity });
    } else if (node.kind === "task") {
      const task = data.tasks.find((task) => task.id === node.resourceId);
      if (task) open({ kind: "taskDetail", task });
    } else if (node.kind === "run") {
      const run = data.runs.find((run) => run.id === node.resourceId);
      if (run) open({ kind: "run", run, node: data.node.id });
    }
  };
  return (
    <section className="collab-graph" aria-label={copy.graph}>
      <header className="collab-channel-heading">
        <h1>{copy.graph}</h1>
        {channel && (
          <button type="button" onClick={() => visitChannel(channel)}>
            {copy.home}
          </button>
        )}
      </header>
      <div className="collab-tabs" role="group" aria-label={copy.graphMode}>
        <button
          type="button"
          aria-pressed={mode === "relationships"}
          onClick={() => setMode("relationships")}
        >
          {copy.relationships}
        </button>
        {data.access.kind === "operator" && (
          <button
            type="button"
            aria-pressed={mode === "topology"}
            onClick={() => setMode("topology")}
          >
            {copy.topology}
          </button>
        )}
      </div>
      {mode === "topology" && data.access.kind === "operator" ? (
        <>
          <MeshView data={data} discovery={discovery} runs={runs} />
          <div className="collab-run-grid">
            {runs.map((item) => (
              <button
                type="button"
                className="collab-task"
                key={`${item.node}:${item.run.id}`}
                onClick={() => open({ kind: "run", ...item })}
              >
                <Badge
                  value={
                    item.run.control === "PAUSED" ? "PAUSED" : item.run.phase
                  }
                />
                {agentLabel(item.node, {
                  id: item.run.agent_id,
                  version: item.run.agent_version,
                })}
                <small>
                  <ReferenceName id={item.node} />
                </small>
              </button>
            ))}
          </div>
        </>
      ) : (
        <>
          <p className="muted" role="note">
            {copy.localSnapshot}
          </p>
          <label className="collab-focus">
            {copy.graphFocus}
            <select
              value={root ? entityKey(data.node.id, "agent", root) : ""}
              onChange={(event) => {
                setFocus(event.target.value);
                setExpanded([]);
                setSelectedId("");
              }}
            >
              <option value="">—</option>
              {agents.map((entry) => (
                <option
                  value={entityKey(data.node.id, "agent", entry)}
                  key={entityKey(data.node.id, "agent", entry)}
                >
                  {local(entry.name)} · v{entry.version}
                </option>
              ))}
            </select>
          </label>
          {!graph || !selected ? (
            <p role="status">{focus ? copy.unavailable : copy.noAgent}</p>
          ) : (
            <>
              <div className="collab-tabs" role="group" aria-label={copy.graph}>
                <button
                  type="button"
                  aria-pressed={view === "graph"}
                  onClick={() => setView("graph")}
                >
                  {copy.graphCanvas}
                </button>
                <button
                  type="button"
                  aria-pressed={view === "list"}
                  onClick={() => setView("list")}
                >
                  {copy.graphList}
                </button>
              </div>
              <fieldset className="agent-graph-filters">
                <legend>{copy.filters}</legend>
                {graphKinds.map((kind) => (
                  <label key={kind}>
                    <input
                      type="checkbox"
                      checked={kinds.includes(kind)}
                      onChange={(event) =>
                        setKinds((current) =>
                          event.target.checked
                            ? [...current, kind]
                            : current.filter((value) => value !== kind),
                        )
                      }
                    />
                    {graphText.types[kind]}
                  </label>
                ))}
              </fieldset>
              <label>
                <input
                  type="checkbox"
                  checked={runtime}
                  onChange={(event) => setRuntime(event.target.checked)}
                />
                {copy.runtime}
              </label>
              {(graph.omitted > 0 || graph.omittedEdges > 0) && (
                <p className="notice" role="status">
                  {copy.omitted}: {graph.omitted} / {graph.omittedEdges}
                </p>
              )}
              <div className="collab-graph-body">
                <div>
                  {view === "graph" ? (
                    <Suspense fallback={<p role="status">{copy.processing}</p>}>
                      <CytoscapeCanvas
                        key={graph.rootId}
                        graph={graph}
                        selectedId={selected.id}
                        label={label}
                        select={setSelectedId}
                        copy={graphText}
                      />
                    </Suspense>
                  ) : (
                    <div className="table-wrap">
                      <table aria-label={copy.graphList}>
                        <thead>
                          <tr>
                            <th>{graphText.entity}</th>
                            <th>{graphText.relationships}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {graph.nodes.map((node) => (
                            <tr key={node.id}>
                              <th scope="row">
                                <button
                                  type="button"
                                  onClick={() => setSelectedId(node.id)}
                                >
                                  {label(node)}
                                </button>
                                {!node.available && (
                                  <small>{copy.unavailable}</small>
                                )}
                              </th>
                              <td>
                                {graph.edges
                                  .filter(
                                    (edge) =>
                                      edge.source === node.id ||
                                      edge.target === node.id,
                                  )
                                  .map((edge) => (
                                    <div key={edge.id}>
                                      {label(
                                        graph.nodes.find(
                                          (value) => value.id === edge.source,
                                        )!,
                                      )}{" "}
                                      → {graphText.relations[edge.relation]} →{" "}
                                      {label(
                                        graph.nodes.find(
                                          (value) => value.id === edge.target,
                                        )!,
                                      )}
                                    </div>
                                  ))}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                </div>
                <aside
                  className="collab-selection"
                  aria-label={graphText.selection}
                >
                  <h2>{label(selected)}</h2>
                  <Badge value={selected.kind} />
                  {selected.status && <Badge value={selected.status} />}
                  <div className="button-row">
                    <button
                      type="button"
                      disabled={!selected.available}
                      onClick={() => inspect(selected)}
                    >
                      {copy.details}
                    </button>
                    <button
                      type="button"
                      disabled={
                        !selected.available ||
                        selected.id === graph.rootId ||
                        expanded.includes(selected.id)
                      }
                      onClick={() =>
                        setExpanded((current) => [...current, selected.id])
                      }
                    >
                      {copy.expand}
                    </button>
                  </div>
                  <h3>{copy.relatedChannels}</h3>
                  {related.length === 0 ? (
                    <p>{copy.noRelated}</p>
                  ) : (
                    related.map((workspace) => (
                      <button
                        type="button"
                        className="collab-channel-link"
                        key={workspace.id}
                        onClick={() => visitChannel(workspace.id)}
                      >
                        # {workspace.title}
                      </button>
                    ))
                  )}
                </aside>
              </div>
            </>
          )}
        </>
      )}
    </section>
  );
}
