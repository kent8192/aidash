import { Button } from "../components/ui/button";
import { NativeSelect } from "../components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../components/ui/table";
import { cn } from "../lib/utils";
import { ReferenceName } from "../record-view";
import { lazy, Suspense, useState } from "react";
import { Check, Loading, Notice, ScreenHeader } from "../components/patterns";
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
  const tab = (pressed: boolean) =>
    cn(
      "-mb-px h-9 border-b-2 border-transparent px-1 text-[13px] font-medium text-muted-foreground transition-colors duration-150 hover:text-foreground",
      pressed && "border-brand-mark text-foreground",
    );
  const segment = (pressed: boolean) =>
    cn(
      "h-6 rounded-sm px-2.5 text-xs font-medium text-muted-foreground transition-colors duration-150 hover:text-foreground",
      pressed && "bg-raised text-foreground shadow-sm",
    );
  return (
    <section
      className="flex h-full min-h-0 flex-col bg-background"
      aria-label={copy.graph}
    >
      <ScreenHeader
        title={copy.graph}
        actions={
          channel && (
            <Button
              variant="outline"
              size="sm"
              type="button"
              onClick={() => visitChannel(channel)}
            >
              {copy.home}
            </Button>
          )
        }
      />
      <div
        className="flex shrink-0 items-end gap-5 border-b border-border px-4"
        role="group"
        aria-label={copy.graphMode}
      >
        <button
          type="button"
          className={tab(mode === "relationships")}
          aria-pressed={mode === "relationships"}
          onClick={() => setMode("relationships")}
        >
          {copy.relationships}
        </button>
        {data.access.kind === "operator" && (
          <button
            type="button"
            className={tab(mode === "topology")}
            aria-pressed={mode === "topology"}
            onClick={() => setMode("topology")}
          >
            {copy.topology}
          </button>
        )}
      </div>
      {mode === "topology" && data.access.kind === "operator" ? (
        <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-auto p-4">
          <MeshView data={data} discovery={discovery} runs={runs} />
          <div className="collab-run-grid grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-2">
            {runs.map((item) => (
              <Button
                variant="outline"
                type="button"
                className="collab-task h-auto justify-start gap-2 py-2 text-left"
                key={`${item.node}:${item.run.id}`}
                onClick={() => open({ kind: "run", ...item })}
              >
                <Badge
                  value={
                    item.run.control === "PAUSED" ? "PAUSED" : item.run.phase
                  }
                />
                <span className="min-w-0 flex-1 truncate">
                  {agentLabel(item.node, {
                    id: item.run.agent_id,
                    version: item.run.agent_version,
                  })}
                </span>
                <small className="font-mono text-[11px] text-faint">
                  <ReferenceName id={item.node} />
                </small>
              </Button>
            ))}
          </div>
        </div>
      ) : (
        <>
          <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b border-border px-4 py-2">
            <label className="flex items-center gap-2 text-xs text-muted-foreground">
              {copy.graphFocus}
              <NativeSelect
                wrapperClassName="w-56"
                className="h-7 text-xs"
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
              </NativeSelect>
            </label>
            {graph && selected && (
              <>
                <div
                  className="inline-flex items-center gap-0.5 rounded-md border border-border-strong bg-surface p-0.5"
                  role="group"
                  aria-label={copy.graph}
                >
                  <button
                    type="button"
                    className={segment(view === "graph")}
                    aria-pressed={view === "graph"}
                    onClick={() => setView("graph")}
                  >
                    {copy.graphCanvas}
                  </button>
                  <button
                    type="button"
                    className={segment(view === "list")}
                    aria-pressed={view === "list"}
                    onClick={() => setView("list")}
                  >
                    {copy.graphList}
                  </button>
                </div>
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
                      {graphText.types[kind]}
                    </Check>
                  ))}
                </fieldset>
                <Check
                  className="text-xs text-muted-foreground"
                  checked={runtime}
                  onChange={(event) => setRuntime(event.target.checked)}
                >
                  <span className="inline-flex items-center gap-1.5">
                    <span
                      aria-hidden
                      className="w-5 border-t border-dashed border-edge-strong"
                    />
                    {copy.runtime}
                  </span>
                </Check>
              </>
            )}
            <p className="ml-auto text-[11px] text-faint" role="note">
              {copy.localSnapshot}
            </p>
          </div>
          {graph && (graph.omitted > 0 || graph.omittedEdges > 0) && (
            <Notice
              tone="warning"
              role="status"
              className="shrink-0 rounded-none border-x-0 border-t-0 px-4 py-1.5"
            >
              {copy.omitted}:{" "}
              <span className="font-mono">
                {graph.omitted} / {graph.omittedEdges}
              </span>
            </Notice>
          )}
          {!graph || !selected ? (
            <p role="status" className="p-4 text-xs text-muted-foreground">
              {focus ? copy.unavailable : copy.noAgent}
            </p>
          ) : (
            <div className="flex min-h-0 flex-1 flex-col overflow-auto lg:flex-row lg:overflow-hidden">
              <div className="canvas-grid relative min-h-[320px] min-w-0 flex-1 lg:min-h-0">
                {view === "graph" ? (
                  <Suspense
                    fallback={
                      <Loading className="p-4">{copy.processing}</Loading>
                    }
                  >
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
                  <div className="h-full overflow-auto bg-surface">
                    <Table aria-label={copy.graphList}>
                      <TableHeader>
                        <TableRow>
                          <TableHead>{graphText.entity}</TableHead>
                          <TableHead>{graphText.relationships}</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {graph.nodes.map((node) => (
                          <TableRow
                            key={node.id}
                            data-state={
                              node.id === selected.id ? "selected" : undefined
                            }
                          >
                            <TableHead
                              scope="row"
                              className="h-auto whitespace-normal py-1.5 align-top"
                            >
                              <Button
                                variant="ghost"
                                size="sm"
                                type="button"
                                className="-ml-2"
                                onClick={() => setSelectedId(node.id)}
                              >
                                {label(node)}
                              </Button>
                              {!node.available && (
                                <small className="block text-[11px] font-normal text-faint">
                                  {copy.unavailable}
                                </small>
                              )}
                            </TableHead>
                            <TableCell className="py-1.5 align-top text-xs text-muted-foreground">
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
                            </TableCell>
                          </TableRow>
                        ))}
                      </TableBody>
                    </Table>
                  </div>
                )}
              </div>
              <aside
                className="collab-selection flex shrink-0 flex-col gap-3 self-stretch border-t border-border bg-surface p-4 lg:w-[340px] lg:overflow-auto lg:border-l lg:border-t-0"
                aria-label={graphText.selection}
              >
                <h2 className="text-[15px] font-semibold text-foreground [overflow-wrap:anywhere]">
                  {label(selected)}
                </h2>
                <div className="flex flex-wrap gap-1.5">
                  <Badge value={selected.kind} />
                  {selected.status && <Badge value={selected.status} />}
                </div>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    type="button"
                    disabled={!selected.available}
                    onClick={() => inspect(selected)}
                  >
                    {copy.details}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
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
                  </Button>
                </div>
                <h3 className="border-t border-border pt-3 text-[11px] font-medium text-faint">
                  {copy.relatedChannels}
                </h3>
                {related.length === 0 ? (
                  <p className="text-xs text-muted-foreground">
                    {copy.noRelated}
                  </p>
                ) : (
                  <div className="flex flex-col gap-0.5">
                    {related.map((workspace) => (
                      <Button
                        variant="ghost"
                        size="sm"
                        type="button"
                        className="collab-channel-link w-full justify-start"
                        key={workspace.id}
                        onClick={() => visitChannel(workspace.id)}
                      >
                        # {workspace.title}
                      </Button>
                    ))}
                  </div>
                )}
              </aside>
            </div>
          )}
        </>
      )}
    </section>
  );
}
