import {
  graphTokens,
  observeGraphTheme,
  onThemeChange,
  type GraphTokens,
} from "./graph-theme";
import { Button } from "../components/ui/button";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import cytoscape, {
  type Core,
  type ElementDefinition,
  type Layouts,
  type StylesheetJson,
} from "cytoscape";
import { Crosshair, Maximize, Minus, Plus, Scan } from "lucide-react";
import { cn } from "../lib/utils";
import { statusTone } from "../ui";
import { useGraphFit } from "../graph-fit-view";
import { cytoscapeFit } from "./graph-camera";
import type { MeshCopy } from "./mesh-copy";
import { floatingToolbar, MeshIcon } from "./mesh-icons";
import { Dot } from "../components/patterns";
import {
  resourceKey,
  type MeshGraph,
  type MeshKind,
  type MeshMode,
  type MeshNode,
} from "./mesh-model";
import {
  graphRegionPositions,
  graphRegions,
  regionForNode,
} from "./graph-regions";

export type MeshLayout = "structured" | "force" | "circle";

const pillKinds: Partial<Record<MeshKind, true>> = {
  tool: true,
  model: true,
  skill: true,
};
/** Canvas tiles: Cytoscape draws the tile, the label layer draws its text at the same size. */
function tileSize(kind: MeshKind) {
  if (kind === "agent") return { w: 176, h: 48 };
  if (pillKinds[kind]) return { w: 132, h: 28 };
  return { w: 156, h: 44 };
}
const regionInset = { x: 12, y: 8 };

function elements(
  graph: MeshGraph,
  localNode: string,
  showNodeFrames: boolean,
  label: (node: MeshNode) => string,
  copy: MeshCopy,
): ElementDefinition[] {
  const positions = graphRegionPositions(graph, localNode);
  const regions = graphRegions(graph, localNode);
  return [
    ...regions.map((region) => ({
      data: { id: region.id, regionKind: region.kind, nodeId: region.nodeId },
      classes: `mesh-group region-${region.kind} ${region.kind === "execution" && !showNodeFrames ? "region-hidden" : ""}`,
    })),
    ...graph.nodes.map((n) => {
      const size = tileSize(n.kind);
      return {
        data: {
          id: n.id,
          label: label(n),
          kind: n.kind,
          parent: regionForNode(n)?.id,
          w: size.w,
          h: size.h,
        },
        position: positions[n.id] ?? { x: 500, y: 400 },
        classes: [
          "mesh-node",
          pillKinds[n.kind] ? "pill" : "",
          n.available ? "" : "unavailable",
        ].join(" "),
      };
    }),
    ...graph.edges.map((e) => ({
      data: {
        id: e.id,
        source: e.source,
        target: e.target,
        fullLabel: copy.relations[e.relation],
        layer: e.layer,
        relation: e.relation,
      },
    })),
  ];
}

const handoff = ["delegates", "coordinates", "assigned", "executes", "creates"]
  .map((relation) => `edge[relation = "${relation}"]`)
  .join(", ");
const structural = ["contains", "member", "hosts", "participates"]
  .map((relation) => `edge[relation = "${relation}"]`)
  .join(", ");

function meshStylesheet(t: GraphTokens): StylesheetJson {
  return [
    {
      selector: "node.mesh-node",
      style: {
        shape: "round-rectangle",
        "corner-radius": "10",
        width: "data(w)",
        height: "data(h)",
        "background-color": t.surface,
        "border-width": 1,
        "border-color": t.borderStrong,
        "overlay-opacity": 0,
        "transition-property": "opacity",
        "transition-duration": 150,
      },
    },
    {
      selector: "node.pill",
      style: { "corner-radius": "14", "border-color": t.border },
    },
    {
      selector: "node.unavailable",
      style: { "border-style": "dashed", "border-color": t.faint },
    },
    {
      selector: "node.mesh-group",
      style: {
        shape: "round-rectangle",
        "corner-radius": "10",
        "background-color": t.raised,
        "background-opacity": 0.28,
        "border-color": t.faint,
        "border-style": "dashed",
        "border-width": 1,
        "border-opacity": 0.7,
        padding: "36px",
        "min-width": "190px",
        "min-height": "110px",
        "overlay-opacity": 0,
      },
    },
    {
      selector: "node.region-shared",
      style: { "background-opacity": 0.14 },
    },
    {
      selector: "node.region-configuration",
      style: { "background-opacity": 0, "border-opacity": 0.45 },
    },
    {
      selector: "node.region-hidden",
      style: { "border-opacity": 0, "background-opacity": 0 },
    },
    {
      selector: "edge",
      style: {
        width: 1,
        "line-color": t.edge,
        "target-arrow-color": t.edge,
        "target-arrow-shape": "none",
        "arrow-scale": 0.7,
        "curve-style": "bezier",
        "control-point-step-size": 40,
        "source-distance-from-node": 2,
        "target-distance-from-node": 3,
        "font-size": 10,
        "font-family": t.mono,
        color: t.muted,
        "text-background-color": t.background,
        "text-background-opacity": 1,
        "text-background-padding": "2px",
        "text-background-shape": "roundrectangle",
        "text-rotation": "none",
        "overlay-opacity": 0,
      },
    },
    {
      selector: handoff,
      style: {
        width: 1.25,
        "line-color": t.edgeStrong,
        "target-arrow-color": t.edgeStrong,
        "target-arrow-shape": "triangle",
      },
    },
    {
      selector: 'edge[relation = "goal"], edge[relation = "produces"]',
      style: { "target-arrow-shape": "triangle" },
    },
    {
      selector: 'edge[relation = "depends"]',
      style: {
        width: 1.25,
        "line-style": "dashed",
        "line-dash-pattern": [5, 4],
        "line-color": t.warning,
        "target-arrow-color": t.warning,
        "target-arrow-shape": "triangle",
      },
    },
    {
      selector: 'edge[relation = "tool"], edge[relation = "skill"]',
      style: { width: 0.75 },
    },
    {
      selector: 'edge[relation = "model"]',
      style: { "line-style": "dotted", width: 1 },
    },
    {
      selector: structural,
      style: { width: 0.75, "line-color": t.borderStrong },
    },
    {
      selector: 'edge[layer = "federation"]',
      style: {
        "line-style": "dashed",
        "line-dash-pattern": [6, 5],
        "line-color": t.edge,
        "target-arrow-shape": "none",
      },
    },
    {
      selector: "node.is-selected",
      style: {
        "border-color": t.brandMark,
        "outline-width": 2,
        "outline-offset": 2,
        "outline-color": t.brandMark,
        "outline-opacity": 0.9,
        "underlay-color": t.brandMark,
        "underlay-opacity": 0.12,
        "underlay-padding": 9,
        "underlay-shape": "round-rectangle",
      },
    },
    {
      selector: "node.mesh-group.is-selected",
      style: {
        "border-color": t.brandMark,
        "border-opacity": 1,
        "outline-width": 0,
        "underlay-opacity": 0,
      },
    },
    {
      selector: "edge.is-connected",
      style: {
        label: "data(fullLabel)",
        width: 1.6,
        "line-color": t.foreground,
        "target-arrow-color": t.foreground,
      },
    },
    {
      selector: 'edge.is-connected[relation = "depends"]',
      style: { "line-color": t.warning, "target-arrow-color": t.warning },
    },
    { selector: ".is-dimmed", style: { opacity: 0.28 } },
  ];
}

export function MeshCanvas({
  graph,
  mode,
  layout,
  selectedId,
  focusId,
  select,
  label,
  status,
  copy,
  localNode,
  showNodeFrames,
  actions,
}: {
  graph: MeshGraph;
  mode: MeshMode;
  layout: MeshLayout;
  selectedId: string;
  /** Explicit selection: its neighbourhood stays bright, the rest dims. */
  focusId: string;
  select: (id: string) => void;
  label: (node: MeshNode) => string;
  status: (node: MeshNode) => string;
  copy: MeshCopy;
  localNode: string;
  showNodeFrames: boolean;
  actions: ReactNode;
}) {
  const host = useRef<HTMLDivElement>(null);
  const needsFraming = useRef(true);
  const fitDescription = useId();
  const mini = useRef<HTMLCanvasElement>(null);
  const instance = useRef<Core | null>(null);
  const runningLayout = useRef<Layouts | null>(null);
  const selectRef = useRef(select);
  const graphRef = useRef(graph);
  const showFramesRef = useRef(showNodeFrames);
  const positions = useRef(new Map<string, { x: number; y: number }>());
  const resetKey = useRef("");
  const miniBounds = useRef({ x: 0, y: 0, scale: 1 });
  const [camera, setCamera] = useState<{
    zoom: number;
    x: number;
    y: number;
    points: Record<string, { x: number; y: number }>;
    regions: Record<string, { x: number; y: number; w: number }>;
  }>({ zoom: 1, x: 0, y: 0, points: {}, regions: {} });
  const fit = useGraphFit(
    host,
    () => {
      const cy = instance.current;
      if (!cy || !host.current || !graph.nodes.length) return null;
      if (graph.nodes.some((node) => cy.getElementById(node.id).empty()))
        return null;
      return cytoscapeFit(cy, host.current, 0.12);
    },
    (camera) => {
      needsFraming.current = false;
      instance.current?.viewport({
        zoom: camera.zoom,
        pan: { x: camera.x, y: camera.y },
      });
    },
    `${mode}:${layout}`,
  );
  useEffect(() => {
    if (needsFraming.current && fit.available && graph.nodes.length)
      fit.request();
  });
  useEffect(() => {
    selectRef.current = select;
    graphRef.current = graph;
    showFramesRef.current = showNodeFrames;
  }, [select, graph, showNodeFrames, localNode]);
  useEffect(() => {
    if (!host.current) return;
    const cy = cytoscape({
      container: host.current,
      elements: [],
      minZoom: 0.12,
      maxZoom: 2.5,
      boxSelectionEnabled: false,
    });
    instance.current = cy;
    const stopTheme = observeGraphTheme(cy, meshStylesheet);
    let tokens = graphTokens();
    let frame = 0;
    const update = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        const pan = cy.pan();
        setCamera({
          zoom: cy.zoom(),
          x: pan.x,
          y: pan.y,
          points: Object.fromEntries(
            cy
              .nodes()
              .not(":parent")
              .nodes()
              .map((n) => [n.id(), { ...n.position() }]),
          ),
          regions: Object.fromEntries(
            cy.nodes(".mesh-group").map((n) => {
              const bounds = n.boundingBox({
                includeLabels: false,
                includeOverlays: false,
                includeUnderlays: false,
              });
              return [
                n.id(),
                {
                  x: bounds.x1 + regionInset.x,
                  y: bounds.y1 + regionInset.y,
                  w: bounds.w - 2 * regionInset.x,
                },
              ];
            }),
          ),
        });
        cy.nodes()
          .not(":parent")
          .forEach((n) => {
            positions.current.set(n.id(), { ...n.position() });
          });
        const canvas = mini.current;
        const ctx = canvas?.getContext("2d");
        if (!canvas || !ctx) return;
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        if (cy.nodes().empty()) return;
        const bounds = cy.elements().boundingBox();
        const scale = Math.min(
          (canvas.width - 16) / Math.max(bounds.w, 1),
          (canvas.height - 16) / Math.max(bounds.h, 1),
        );
        miniBounds.current = { x: bounds.x1, y: bounds.y1, scale };
        const project = (point: { x: number; y: number }) => ({
          x: (point.x - bounds.x1) * scale + 8,
          y: (point.y - bounds.y1) * scale + 8,
        });
        ctx.strokeStyle = tokens.edge;
        ctx.lineWidth = 0.5;
        cy.edges().forEach((e) => {
          const a = project(e.source().position());
          const b = project(e.target().position());
          ctx.beginPath();
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
          ctx.stroke();
        });
        cy.nodes()
          .not(":parent")
          .forEach((n) => {
            const p = project(n.position());
            ctx.fillStyle = n.hasClass("is-selected")
              ? tokens.brandMark
              : tokens.muted;
            ctx.beginPath();
            ctx.arc(p.x, p.y, 1.8, 0, Math.PI * 2);
            ctx.fill();
          });
        const extent = cy.extent();
        const corner = project({ x: extent.x1, y: extent.y1 });
        ctx.strokeStyle = tokens.brandMark;
        ctx.lineWidth = 1;
        ctx.strokeRect(corner.x, corner.y, extent.w * scale, extent.h * scale);
      });
    };
    const stopMinimapTheme = onThemeChange(() => {
      tokens = graphTokens();
      update();
    });
    cy.on("tap", "node", (event) => {
      if (graphRef.current.nodes.some((n) => n.id === event.target.id()))
        selectRef.current(event.target.id());
      else if (
        showFramesRef.current &&
        event.target.data("regionKind") === "execution"
      )
        selectRef.current(
          resourceKey(
            event.target.data("nodeId"),
            "remote",
            event.target.data("nodeId"),
          ),
        );
    });
    cy.on("pan zoom position add remove layoutstop", update);
    const observer = new ResizeObserver(() => {
      cy.resize();
      update();
    });
    observer.observe(host.current);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      runningLayout.current?.stop();
      stopTheme();
      stopMinimapTheme();
      cy.destroy();
      instance.current = null;
    };
  }, []);
  useEffect(() => {
    const cy = instance.current;
    if (!cy) return;
    const definitions = elements(graph, localNode, showNodeFrames, label, copy);
    const desired = new Set(definitions.map((e) => e.data.id));
    const key = `${mode}:${layout}`;
    const reset = key !== resetKey.current;
    runningLayout.current?.stop();
    cy.batch(() => {
      // Detach surviving children before removing a filtered compound parent.
      cy.nodes()
        .filter((n) => desired.has(n.id()))
        .forEach((n) => {
          if (n.isChild() && !desired.has(n.parent()[0]?.id()))
            n.move({ parent: null });
        });
      cy.elements()
        .filter((e) => !desired.has(e.id()))
        .remove();
      for (const d of definitions) {
        const existing = cy.getElementById(d.data.id!);
        if (existing.empty()) {
          const saved = reset ? undefined : positions.current.get(d.data.id!);
          cy.add({ ...d, position: saved ?? d.position });
        } else {
          if (existing.isNode() && existing.parent()[0]?.id() !== d.data.parent)
            existing.move({ parent: d.data.parent ?? null });
          existing.data(d.data);
          existing.classes(d.classes ?? "");
          if (reset && d.position && !existing.isParent())
            existing.position(d.position);
        }
      }
    });
    if (reset) {
      needsFraming.current = true;
      resetKey.current = key;
      // Perspectives change the canvas height (timeline) and width.
      // Measure that viewport before fitting, rather than waiting for ResizeObserver.
      cy.resize();
      if (layout !== "structured") {
        const run = cy.layout(
          layout === "force"
            ? {
                name: "cose",
                animate: false,
                fit: false,
                padding: 65,
                nodeRepulsion: () => 32000,
                idealEdgeLength: () => 170,
                nodeOverlap: 40,
                numIter: 450,
              }
            : {
                name: "circle",
                fit: false,
                padding: 70,
                spacingFactor: 1.5,
                animate: false,
              },
        );
        runningLayout.current = run;
        run.run();
      }
    }
    cy.emit("position");
  }, [graph, mode, layout, label, copy, localNode, showNodeFrames]);
  const focusVisible = graph.nodes.some((node) => node.id === focusId);
  const bright = new Set<string>([focusId]);
  for (const edge of graph.edges) {
    if (edge.source === focusId) bright.add(edge.target);
    if (edge.target === focusId) bright.add(edge.source);
  }
  useEffect(() => {
    const cy = instance.current;
    if (!cy) return;
    cy.batch(() => {
      cy.elements().removeClass("is-selected is-connected is-dimmed");
      const selected = cy.getElementById(selectedId);
      selected.addClass("is-selected");
      selected.connectedEdges().addClass("is-connected");
      for (const region of graphRegions(graph, localNode))
        if (
          region.kind === "execution" &&
          resourceKey(region.nodeId, "remote", region.nodeId) === selectedId
        )
          cy.getElementById(region.id).addClass("is-selected");
      const focused = cy.getElementById(focusId);
      if (focusId && !focused.empty() && !focused.isParent())
        cy.elements()
          .not(".mesh-group")
          .difference(focused.closedNeighborhood())
          .addClass("is-dimmed");
    });
    cy.emit("position");
  }, [selectedId, focusId, graph, mode, layout, label, copy, localNode]);
  const zoom = (factor: number) => {
    fit.cancel();
    needsFraming.current = false;
    const cy = instance.current;
    if (cy)
      cy.zoom({
        level: Math.max(
          cy.minZoom(),
          Math.min(cy.maxZoom(), cy.zoom() * factor),
        ),
        renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 },
      });
  };
  return (
    <div className="absolute inset-0 overflow-hidden">
      <div
        ref={host}
        className="collab-cytoscape mesh-canvas h-full w-full touch-none"
        role="img"
        aria-label={copy.canvas}
      />
      <div
        className="mesh-label-layer pointer-events-none absolute left-0 top-0 origin-top-left"
        style={{
          transform: `translate(${camera.x}px, ${camera.y}px) scale(${camera.zoom})`,
        }}
        onWheel={(event) =>
          host.current?.dispatchEvent(
            new WheelEvent("wheel", {
              deltaX: event.deltaX,
              deltaY: event.deltaY,
              deltaMode: event.deltaMode,
              clientX: event.clientX,
              clientY: event.clientY,
              ctrlKey: event.ctrlKey,
              bubbles: true,
              cancelable: true,
            }),
          )
        }
      >
        {graphRegions(graph, localNode).map((region) => {
          const point = camera.regions[region.id];
          if (!point || (region.kind === "execution" && !showNodeFrames))
            return null;
          const workspace = graph.nodes.find(
            (node) =>
              node.kind === "workspace" &&
              node.nodeId === region.nodeId &&
              node.resourceId === region.workspaceId,
          );
          const heading =
            region.kind === "execution"
              ? `${copy.executionRegion} · ${region.nodeId}`
              : region.kind === "shared"
                ? `${copy.sharedData} · ${workspace ? label(workspace) : region.workspaceId} · ${copy.homeNode}: ${region.nodeId}`
                : `${copy.configurationReferences} · ${region.nodeId}`;
          const style = {
            left: point.x,
            top: point.y,
            maxWidth: Math.max(point.w, 80),
          };
          const content =
            region.kind === "execution" ? (
              <>
                <span className="truncate text-muted-foreground">
                  {region.nodeId}
                </span>
                <span className="shrink-0 font-sans text-faint">
                  {copy.executionRegion}
                </span>
              </>
            ) : region.kind === "shared" ? (
              <>
                <span className="shrink-0 font-sans text-faint">
                  {copy.sharedData}
                </span>
                <span className="truncate font-sans text-muted-foreground">
                  {workspace ? label(workspace) : region.workspaceId}
                </span>
                <span className="truncate text-faint">
                  {`${copy.homeNode}: ${region.nodeId}`}
                </span>
              </>
            ) : (
              <>
                <span className="shrink-0 font-sans text-faint">
                  {copy.configurationReferences}
                </span>
                <span className="truncate text-faint">{region.nodeId}</span>
              </>
            );
          return region.kind === "execution" ? (
            <button
              type="button"
              className={cn(
                "mesh-region-label mesh-region-execution-label pointer-events-auto absolute flex h-5 items-center gap-2 whitespace-nowrap rounded-sm px-1 font-mono text-[11px] transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring",
                resourceKey(region.nodeId, "remote", region.nodeId) ===
                  selectedId && "bg-brand-soft",
              )}
              data-mesh-region={region.id}
              key={region.id}
              style={style}
              onClick={() =>
                select(resourceKey(region.nodeId, "remote", region.nodeId))
              }
              title={heading}
            >
              {content}
            </button>
          ) : (
            <span
              className="mesh-region-label absolute flex h-5 items-center gap-2 whitespace-nowrap px-1 font-mono text-[11px]"
              data-mesh-region={region.id}
              key={region.id}
              style={style}
              title={heading}
            >
              {content}
            </span>
          );
        })}
        {graph.nodes.map((node) => {
          const point = camera.points[node.id];
          if (!point) return null;
          const size = tileSize(node.kind);
          const name = label(node);
          const tone = node.status ? statusTone(node.status) : "neutral";
          const pill = pillKinds[node.kind];
          return (
            <button
              type="button"
              key={node.id}
              data-mesh-node={node.id}
              className={cn(
                "mesh-node-label pointer-events-auto absolute flex -translate-x-1/2 -translate-y-1/2 items-center text-left transition-[background-color,opacity] duration-150 hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring motion-reduce:transition-none",
                pill ? "gap-1.5 rounded-full px-3" : "gap-2 rounded-lg px-2.5",
                selectedId === node.id && "selected",
                focusVisible && !bright.has(node.id) && "opacity-35",
              )}
              aria-pressed={selectedId === node.id}
              onClick={() => select(node.id)}
              onFocus={(event) => {
                if (!event.currentTarget.matches(":focus-visible")) return;
                const cy = instance.current;
                fit.cancel();
                needsFraming.current = false;
                if (cy) cy.center(cy.getElementById(node.id));
              }}
              style={{
                left: point.x,
                top: point.y,
                width: size.w,
                height: size.h,
              }}
              title={`${name} · ${copy.kinds[node.kind]}`}
            >
              {pill ? (
                <>
                  <MeshIcon
                    kind={node.kind}
                    size={12}
                    className="shrink-0 text-faint"
                  />
                  <strong className="truncate font-mono text-[11px] font-normal text-muted-foreground">
                    {name}
                  </strong>
                </>
              ) : (
                <>
                  {node.kind === "agent" ? (
                    <span
                      aria-hidden="true"
                      className="grid size-7 shrink-0 place-items-center rounded-md border border-border-strong bg-raised text-[11px] font-semibold text-foreground"
                    >
                      {name.trim().slice(0, 1).toUpperCase()}
                    </span>
                  ) : (
                    <MeshIcon
                      kind={node.kind}
                      size={14}
                      className="shrink-0 text-muted-foreground"
                    />
                  )}
                  <span className="grid min-w-0 flex-1 gap-0.5">
                    <strong
                      className={cn(
                        "truncate font-medium leading-tight text-foreground",
                        node.kind === "agent" ? "text-[13px]" : "text-xs",
                      )}
                    >
                      {name}
                    </strong>
                    <span className="flex min-w-0 items-center gap-1.5 text-[11px] leading-tight text-faint">
                      {node.kind !== "agent" && (
                        <span className="mesh-node-kind truncate">
                          {copy.kinds[node.kind]}
                        </span>
                      )}
                      {node.status && (
                        <span className="flex min-w-0 items-center gap-1 text-muted-foreground">
                          <Dot tone={tone} />
                          <span className="truncate">{status(node)}</span>
                        </span>
                      )}
                      {!node.available && (
                        <span className="mesh-node-kind truncate">
                          {copy.missing}
                        </span>
                      )}
                    </span>
                  </span>
                </>
              )}
            </button>
          );
        })}
      </div>
      <button
        type="button"
        className="mesh-minimap absolute bottom-3 right-3 z-10 overflow-hidden rounded-md border border-border-strong bg-surface/90 transition-colors hover:border-edge focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-45 max-md:hidden"
        aria-label={copy.minimap}
        disabled={!graph.nodes.length}
        onClick={(event) => {
          const cy = instance.current;
          if (!cy || cy.nodes().empty()) return;
          if (event.detail === 0) {
            fit.request();
            return;
          }
          fit.cancel();
          needsFraming.current = false;
          const canvas = mini.current;
          if (!canvas) return;
          const rect = canvas.getBoundingClientRect();
          const { x, y, scale } = miniBounds.current;
          const world = {
            x:
              (((event.clientX - rect.left) * canvas.width) / rect.width - 8) /
                scale +
              x,
            y:
              (((event.clientY - rect.top) * canvas.height) / rect.height - 8) /
                scale +
              y,
          };
          cy.pan({
            x: cy.width() / 2 - world.x * cy.zoom(),
            y: cy.height() / 2 - world.y * cy.zoom(),
          });
        }}
      >
        <canvas
          ref={mini}
          width={132}
          height={92}
          className="block h-[92px] w-[132px]"
        />
      </button>
      <div
        className={floatingToolbar}
        role="group"
        aria-label={copy.camera}
      >
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          aria-label={copy.zoomOut}
          title={copy.zoomOut}
          onClick={() => zoom(0.8)}
        >
          <Minus />
        </Button>
        <span className="w-10 text-center font-mono text-[11px] text-muted-foreground tabular max-md:hidden">
          {Math.round(camera.zoom * 100)}%
        </span>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          aria-label={copy.zoomIn}
          title={copy.zoomIn}
          onClick={() => zoom(1.25)}
        >
          <Plus />
        </Button>
        <span aria-hidden="true" className="mx-0.5 h-4 w-px bg-border" />
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2"
          aria-label={copy.fit}
          title={
            fit.available ? copy.fit : `${copy.fit}: ${copy.fitUnavailable}`
          }
          aria-describedby={fitDescription}
          disabled={!fit.available}
          onClick={fit.request}
        >
          <Scan aria-hidden="true" />
          <span>{copy.fit}</span>
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          aria-label={copy.center}
          title={copy.center}
          disabled={!selectedId}
          onClick={() => {
            fit.cancel();
            needsFraming.current = false;
            const cy = instance.current;
            if (cy) cy.center(cy.getElementById(selectedId));
          }}
        >
          <Crosshair />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-7 max-md:hidden"
          aria-label={copy.fullscreen}
          title={copy.fullscreen}
          onClick={() => {
            if (document.fullscreenElement)
              void document.exitFullscreen().catch(() => {});
            else
              void host.current
                ?.closest<HTMLElement>(".mesh-graph")
                ?.requestFullscreen()
                .catch(() => {});
          }}
        >
          <Maximize />
        </Button>
        <span aria-hidden="true" className="mx-0.5 h-4 w-px bg-border" />
        {actions}
      </div>
      <span id={fitDescription} className="sr-only" role="status">
        {fit.available ? "" : copy.fitUnavailable}
      </span>
    </div>
  );
}
