import { useMemo, useState } from "react";
import type { Entry, State } from "./types";
import { Badge, useI18n } from "./ui";
import {
  buildAgentGraph,
  graphKinds,
  type GraphKind,
  type GraphNode,
} from "./agent-graph/model";
import { graphCopy } from "./agent-graph/copy";
import { GraphCanvas } from "./agent-graph/canvas";
import "./agent-graph/style.css";

export function AgentRelationshipGraph({
  entry,
  data,
  open,
}: {
  entry: Entry;
  data: State;
  open: (node: GraphNode) => void;
}) {
  const { locale, local } = useI18n();
  const copy = graphCopy[locale];
  const [view, setView] = useState<"graph" | "list">("graph");
  const [runtime, setRuntime] = useState(true);
  const [kinds, setKinds] = useState<GraphKind[]>([...graphKinds]);
  const [expanded, setExpanded] = useState<string[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const graph = useMemo(
    () =>
      buildAgentGraph(
        {
          nodeId: data.node.id,
          root: { id: entry.id, version: entry.version },
          entries: data.registry,
          runs: data.runs,
          tasks: data.tasks,
        },
        { kinds, runtime, expandedIds: expanded },
      ),
    [
      data.node.id,
      data.registry,
      data.runs,
      data.tasks,
      entry.id,
      entry.version,
      kinds,
      runtime,
      expanded,
    ],
  );
  if (!graph) return <p role="status">{copy.unavailable}</p>;
  const selected =
    graph.nodes.find((node) => node.id === selectedId) ?? graph.nodes[0];
  const label = (node: GraphNode) => {
    const name =
      local(node.name) ||
      node.entity?.id ||
      node.resourceId ||
      copy.types[node.kind];
    return node.entity ? `${name} · v${node.entity.version}` : name;
  };
  const expand = (node: GraphNode) => {
    if (!node.available) return;
    setSelectedId(node.id);
    setExpanded((current) =>
      current.includes(node.id) ? current : [...current, node.id],
    );
  };
  const openNode = (node: GraphNode) => {
    if (node.available) open(node);
  };
  return (
    <section className="agent-graph" aria-label={copy.title}>
      <h4>{copy.title}</h4>
      <p className="muted">{copy.help}</p>
      <div className="agent-graph-controls">
        <button
          type="button"
          aria-pressed={view === "graph"}
          onClick={() => setView("graph")}
        >
          {copy.graph}
        </button>
        <button
          type="button"
          aria-pressed={view === "list"}
          onClick={() => setView("list")}
        >
          {copy.list}
        </button>
        <button
          type="button"
          onClick={() => {
            setExpanded([]);
            setSelectedId(graph.rootId);
          }}
        >
          {copy.collapse}
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
            {copy.types[kind]}
          </label>
        ))}
      </fieldset>
      <div className="agent-graph-legend">
        <span className="agent-graph-configuration">{copy.configuration}</span>
        <label>
          <input
            type="checkbox"
            checked={runtime}
            onChange={(event) => setRuntime(event.target.checked)}
          />
          {copy.runtime}
        </label>
      </div>
      <p className="muted">{copy.snapshot}</p>
      {(graph.omitted > 0 || graph.omittedEdges > 0) && (
        <p role="status" className="notice">
          {copy.hidden}: {graph.omitted} · {copy.hiddenEdges}:{" "}
          {graph.omittedEdges}. {copy.filters}
        </p>
      )}
      {graph.edges.length === 0 && <p role="status">{copy.empty}</p>}
      <div
        className="agent-graph-selection"
        role="group"
        aria-label={copy.selection}
      >
        <strong>{label(selected)}</strong>
        <Badge value={selected.kind} />
        {selected.status && <Badge value={selected.status} />}
        {!selected.available && <span>{copy.missing}</span>}
        <button
          type="button"
          disabled={
            !selected.available ||
            selected.id === graph.rootId ||
            expanded.includes(selected.id)
          }
          onClick={() => expand(selected)}
        >
          {copy.expand}
        </button>
        <button
          type="button"
          disabled={!selected.available}
          onClick={() => openNode(selected)}
        >
          {copy.open}
        </button>
      </div>
      {view === "graph" ? (
        <GraphCanvas
          graph={graph}
          selectedId={selected.id}
          label={label}
          select={setSelectedId}
          copy={copy}
        />
      ) : (
        <div className="agent-graph-table">
          <table aria-label={copy.list}>
            <thead>
              <tr>
                <th>{copy.entity}</th>
                <th>{copy.type}</th>
                <th>{copy.relationships}</th>
                <th>{copy.actions}</th>
              </tr>
            </thead>
            <tbody>
              {graph.nodes.map((node) => (
                <tr key={node.id}>
                  <th scope="row">
                    {label(node)}
                    {!node.available && <small>{copy.missing}</small>}
                  </th>
                  <td>
                    {copy.types[node.kind]}
                    {node.status && <Badge value={node.status} />}
                  </td>
                  <td>
                    <ul>
                      {graph.edges
                        .filter(
                          (edge) =>
                            edge.source === node.id || edge.target === node.id,
                        )
                        .map((edge) => {
                          const from = graph.nodes.find(
                            (value) => value.id === edge.source,
                          )!;
                          const to = graph.nodes.find(
                            (value) => value.id === edge.target,
                          )!;
                          return (
                            <li key={edge.id}>
                              <span
                                className={`agent-graph-layer-${edge.layer}`}
                              >
                                {edge.layer === "configuration"
                                  ? copy.configuration
                                  : copy.runtime}
                              </span>
                              : {label(from)} → {copy.relations[edge.relation]}{" "}
                              → {label(to)}
                            </li>
                          );
                        })}
                    </ul>
                  </td>
                  <td>
                    <div className="agent-graph-row-actions">
                      <button
                        type="button"
                        onClick={() => {
                          setSelectedId(node.id);
                          setView("graph");
                        }}
                      >
                        {copy.selection}
                      </button>
                      <button
                        type="button"
                        disabled={
                          !node.available ||
                          node.id === graph.rootId ||
                          expanded.includes(node.id)
                        }
                        onClick={() => expand(node)}
                      >
                        {copy.expand}
                      </button>
                      <button
                        type="button"
                        disabled={!node.available}
                        aria-label={`${copy.open}: ${label(node)}`}
                        onClick={() => openNode(node)}
                      >
                        {copy.open}
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
