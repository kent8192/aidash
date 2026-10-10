import { Button } from "./components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { cn } from "./lib/utils";
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
import { Check, Notice } from "./components/patterns";
import { GraphCanvas } from "./agent-graph/canvas";

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
    const name = local(node.name) || copy.types[node.kind];
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
  const segment = (pressed: boolean) =>
    cn(
      "h-6 rounded-sm px-2.5 text-xs font-medium text-muted-foreground transition-colors duration-150 hover:text-foreground",
      pressed && "bg-raised text-foreground shadow-sm",
    );
  return (
    <section
      className="agent-graph my-5 flex flex-col gap-3 border-t border-border pt-4"
      aria-label={copy.title}
    >
      <header className="flex flex-col gap-1">
        <h4 className="text-[15px] font-semibold text-foreground">
          {copy.title}
        </h4>
        <p className="text-xs text-muted-foreground">{copy.help}</p>
      </header>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="inline-flex items-center gap-0.5 rounded-md border border-border-strong bg-surface p-0.5">
          <button
            type="button"
            className={segment(view === "graph")}
            aria-pressed={view === "graph"}
            onClick={() => setView("graph")}
          >
            {copy.graph}
          </button>
          <button
            type="button"
            className={segment(view === "list")}
            aria-pressed={view === "list"}
            onClick={() => setView("list")}
          >
            {copy.list}
          </button>
        </div>
        <Button
          variant="ghost"
          size="sm"
          type="button"
          onClick={() => {
            setExpanded([]);
            setSelectedId(graph.rootId);
          }}
        >
          {copy.collapse}
        </Button>
        <fieldset className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
          <legend className="sr-only">{copy.filters}</legend>
          {graphKinds.map((kind) => (
            <Check
              key={kind}
              className="text-xs text-muted-foreground"
              checked={kinds.includes(kind)}
              onChange={(event) =>
                setKinds((current) =>
                  event.target.checked
                    ? [...current, kind]
                    : current.filter((value) => value !== kind),
                )
              }
            >
              {copy.types[kind]}
            </Check>
          ))}
        </fieldset>
        <div className="ml-auto flex flex-wrap items-center gap-3 text-[11px] text-faint">
          <span className="inline-flex items-center gap-1.5">
            <span aria-hidden className="w-6 border-t border-edge-strong" />
            {copy.configuration}
          </span>
          <Check
            className="text-[11px] text-faint"
            checked={runtime}
            onChange={(event) => setRuntime(event.target.checked)}
          >
            <span className="inline-flex items-center gap-1.5">
              <span
                aria-hidden
                className="w-6 border-t border-dashed border-brand-mark"
              />
              {copy.runtime}
            </span>
          </Check>
        </div>
      </div>
      <p className="text-[11px] text-faint">{copy.snapshot}</p>
      {(graph.omitted > 0 || graph.omittedEdges > 0) && (
        <Notice tone="warning" role="status">
          {copy.hidden}: <span className="font-mono">{graph.omitted}</span> ·{" "}
          {copy.hiddenEdges}:{" "}
          <span className="font-mono">{graph.omittedEdges}</span>.{" "}
          {copy.filters}
        </Notice>
      )}
      {graph.edges.length === 0 && (
        <p role="status" className="text-xs text-muted-foreground">
          {copy.empty}
        </p>
      )}
      <div
        className="agent-graph-selection flex flex-wrap items-center gap-2 border-y border-border py-2"
        role="group"
        aria-label={copy.selection}
      >
        <strong className="font-medium text-foreground [overflow-wrap:anywhere]">
          {label(selected)}
        </strong>
        <Badge value={selected.kind} />
        {selected.status && <Badge value={selected.status} />}
        {!selected.available && (
          <span className="text-xs text-faint">{copy.missing}</span>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            type="button"
            disabled={
              !selected.available ||
              selected.id === graph.rootId ||
              expanded.includes(selected.id)
            }
            onClick={() => expand(selected)}
          >
            {copy.expand}
          </Button>
          <Button
            variant="outline"
            size="sm"
            type="button"
            disabled={!selected.available}
            onClick={() => openNode(selected)}
          >
            {copy.open}
          </Button>
        </div>
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
        <div className="max-h-[65vh] overflow-auto rounded-lg border border-border bg-surface">
          <Table aria-label={copy.list}>
            <TableHeader>
              <TableRow>
                <TableHead>{copy.entity}</TableHead>
                <TableHead>{copy.type}</TableHead>
                <TableHead>{copy.relationships}</TableHead>
                <TableHead>{copy.actions}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {graph.nodes.map((node) => (
                <TableRow key={node.id}>
                  <TableHead
                    scope="row"
                    className="h-auto whitespace-normal py-2 align-top text-[13px] font-medium text-foreground"
                  >
                    {label(node)}
                    {!node.available && (
                      <small className="block text-[11px] font-normal text-faint">
                        {copy.missing}
                      </small>
                    )}
                  </TableHead>
                  <TableCell className="py-2 align-top">
                    <span className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                      {copy.types[node.kind]}
                      {node.status && <Badge value={node.status} />}
                    </span>
                  </TableCell>
                  <TableCell className="py-2 align-top">
                    <ul className="flex flex-col gap-1 text-xs text-muted-foreground">
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
                                className={cn(
                                  "text-[11px] font-medium",
                                  edge.layer === "configuration"
                                    ? "text-foreground"
                                    : "text-brand",
                                )}
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
                  </TableCell>
                  <TableCell className="py-2 align-top">
                    <div className="flex flex-wrap gap-1.5">
                      <Button
                        variant="ghost"
                        size="sm"
                        type="button"
                        onClick={() => {
                          setSelectedId(node.id);
                          setView("graph");
                        }}
                      >
                        {copy.selection}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        type="button"
                        disabled={
                          !node.available ||
                          node.id === graph.rootId ||
                          expanded.includes(node.id)
                        }
                        onClick={() => expand(node)}
                      >
                        {copy.expand}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        type="button"
                        disabled={!node.available}
                        aria-label={`${copy.open}: ${label(node)}`}
                        onClick={() => openNode(node)}
                      >
                        {copy.open}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  );
}
