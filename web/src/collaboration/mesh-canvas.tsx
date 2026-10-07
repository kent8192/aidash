import { observeGraphTheme } from "./graph-theme";
import { Button } from "../components/ui/button";
import { useEffect, useId, useRef, useState } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import cytoscape, {
  type Core,
  type ElementDefinition,
  type Layouts,
} from "cytoscape";
import { Crosshair, Maximize, Minus, Plus, Scan } from "lucide-react";
import { useGraphFit } from "../graph-fit-view";
import type { Bounds } from "../graph-fit";
import { cytoscapeFit } from "./graph-camera";
import type { MeshCopy } from "./mesh-copy";
import { meshColors, meshIcons } from "./mesh-icons";
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
const iconUrls = Object.fromEntries(
  Object.entries(meshIcons).map(([kind, Icon]) => [
    kind,
    `data:image/svg+xml;charset=utf-8,${encodeURIComponent(renderToStaticMarkup(<Icon color={meshColors[kind as MeshKind]} width={28} height={28} strokeWidth={1.5} />))}`,
  ]),
);

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
    ...[...graph.nodes].map((n) => ({
      data: {
        id: n.id,
        label: label(n),
        kind: n.kind,
        parent: regionForNode(n)?.id,
        color: meshColors[n.kind],
        icon: iconUrls[n.kind],
      },
      position: positions[n.id] ?? { x: 500, y: 400 },
      classes: ["mesh-node", n.available ? "" : "unavailable"].join(" "),
    })),
    ...graph.edges.map((e) => ({
      data: {
        id: e.id,
        source: e.source,
        target: e.target,
        label:
          graph.edges.length < 30 ||
          ["goal", "delegates", "federation", "depends"].includes(e.relation)
            ? copy.relations[e.relation]
            : "",
        fullLabel: copy.relations[e.relation],
        layer: e.layer,
        relation: e.relation,
      },
    })),
  ];
}

function labelOffset(node: MeshNode) {
  return {
    x: 0,
    y: node.kind === "goal" || node.kind === "workspace" ? 39 : 29,
  };
}

function regionLabelPosition(bounds: Bounds) {
  return { x: (bounds.x1 + bounds.x2) / 2, y: bounds.y1 + 10 };
}

export function MeshCanvas({
  graph,
  mode,
  layout,
  selectedId,
  select,
  label,
  status,
  copy,
  localNode,
  showNodeFrames,
}: {
  graph: MeshGraph;
  mode: MeshMode;
  layout: MeshLayout;
  selectedId: string;
  select: (id: string) => void;
  label: (node: MeshNode) => string;
  status: (node: MeshNode) => string;
  copy: MeshCopy;
  localNode: string;
  showNodeFrames: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);
  const labelLayer = useRef<HTMLDivElement>(null);
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
    regions: Record<string, { x: number; y: number }>;
  }>({ zoom: 1, x: 0, y: 0, points: {}, regions: {} });
  const fit = useGraphFit(
    host,
    () => {
      const cy = instance.current;
      const layer = labelLayer.current;
      if (!cy || !host.current || !layer || !graph.nodes.length) return null;
      const labels = new Map(
        [...layer.querySelectorAll<HTMLElement>("[data-mesh-node]")].map(
          (element) => [element.dataset.meshNode, element],
        ),
      );
      const bounds: Bounds[] = [];
      for (const node of graph.nodes) {
        const rendered = cy.getElementById(node.id);
        if (rendered.empty()) return null;
        if (rendered.isParent()) continue;
        const element = labels.get(node.id);
        if (!element || !element.offsetWidth || !element.offsetHeight)
          return null;
        const point = rendered.position();
        const offset = labelOffset(node);
        bounds.push({
          x1: point.x + offset.x - element.offsetWidth / 2,
          y1: point.y + offset.y,
          x2: point.x + offset.x + element.offsetWidth / 2,
          y2: point.y + offset.y + element.offsetHeight,
        });
      }
      const regionLabels = new Map(
        [...layer.querySelectorAll<HTMLElement>("[data-mesh-region]")].map(
          (element) => [element.dataset.meshRegion, element],
        ),
      );
      for (const region of graphRegions(graph, localNode)) {
        if (region.kind === "execution" && !showNodeFrames) continue;
        const rendered = cy.getElementById(region.id);
        if (rendered.empty()) return null;
        const element = regionLabels.get(region.id);
        if (!element || !element.offsetWidth || !element.offsetHeight)
          return null;
        const point = regionLabelPosition(
          rendered.boundingBox({ includeLabels: false }),
        );
        bounds.push({
          x1: point.x - element.offsetWidth / 2,
          y1: point.y,
          x2: point.x + element.offsetWidth / 2,
          y2: point.y + element.offsetHeight,
        });
      }
      return cytoscapeFit(cy, host.current, 0.12, bounds);
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
      style: [
        {
          selector: "node.mesh-node",
          style: {
            width: 44,
            height: 44,
            "background-color": "#0b2422",
            "border-color": "data(color)",
            "border-width": 1.5,
            "background-image": "data(icon)",
            "background-width": 25,
            "background-height": 25,
            "background-fit": "none",
            "overlay-opacity": 0,
            "underlay-color": "data(color)",
            "underlay-opacity": 0.045,
            "underlay-padding": 7,
            "underlay-shape": "ellipse",
          },
        },
        {
          selector: 'node[kind = "agent"]',
          style: { shape: "hexagon", width: 48, height: 50 },
        },
        {
          selector:
            'node[kind = "task"], node[kind = "artifact"], node[kind = "tool"], node[kind = "skill"], node[kind = "model"]',
          style: {
            shape: "roundrectangle",
            width: 36,
            height: 39,
            "background-width": 22,
            "background-height": 22,
          },
        },
        {
          selector: 'node[kind = "goal"], node[kind = "workspace"]',
          style: {
            width: 61,
            height: 61,
            "background-width": 32,
            "background-height": 32,
            "underlay-padding": 10,
          },
        },
        {
          selector: "node.mesh-group",
          style: {
            shape: "roundrectangle",
            "background-color": "#102e27",
            "background-opacity": 0.16,
            "border-color": "#507b70",
            "border-style": "dashed",
            "border-width": 1,
            padding: "62px",
            "min-width": "190px",
            "min-height": "110px",
            "background-image": "none",
          },
        },
        {
          selector: "node.region-execution",
          style: { "border-color": "#9984bd", "background-color": "#242039" },
        },
        {
          selector: "node.region-shared",
          style: { "border-color": "#61a7a0", "background-color": "#103a36" },
        },
        {
          selector: "node.region-configuration",
          style: { "border-color": "#6f8d96", "background-color": "#182d35" },
        },
        {
          selector: "node.region-hidden",
          style: { "border-opacity": 0, "background-opacity": 0 },
        },
        {
          selector: "node.unavailable",
          style: { "border-style": "dashed", opacity: 0.55 },
        },
        {
          selector: "edge",
          style: {
            label: "data(label)",
            width: 1,
            "line-color": "#517b70",
            "target-arrow-color": "#81a69d",
            "target-arrow-shape": "triangle",
            "arrow-scale": 0.6,
            "curve-style": "bezier",
            "control-point-step-size": 45,
            "font-size": 9,
            "font-family": "DM Sans, Noto Sans JP, sans-serif",
            color: "#93ada6",
            "text-background-color": "#061e19",
            "text-background-opacity": 0.85,
            "text-background-padding": "3px",
            "text-rotation": "none",
            "text-margin-y": -7,
            "text-wrap": "wrap",
            "text-max-width": "95px",
            "source-distance-from-node": 5,
            "target-distance-from-node": 7,
          },
        },
        {
          selector: 'edge[layer = "federation"]',
          style: {
            "line-style": "dashed",
            "line-color": "#c18b62",
            "target-arrow-color": "#c18b62",
            color: "#d4a076",
          },
        },
        {
          selector:
            'edge[relation = "depends"], edge[relation = "tool"], edge[relation = "skill"]',
          style: { "line-style": "dashed" },
        },
        {
          selector: ".is-selected",
          style: {
            "border-width": 3,
            "underlay-opacity": 0.12,
            "underlay-padding": 12,
          },
        },
        {
          selector: "edge.is-connected",
          style: {
            label: "data(fullLabel)",
            width: 1.6,
            "line-color": "#93c5b6",
            "target-arrow-color": "#93c5b6",
          },
        },
      ],
    });
    instance.current = cy;
    const stopTheme = observeGraphTheme(cy, true);
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
              const bounds = n.boundingBox({ includeLabels: false });
              return [n.id(), regionLabelPosition(bounds)];
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
        ctx.clearRect(0, 0, 132, 92);
        if (cy.nodes().empty()) return;
        const bounds = cy.elements().boundingBox();
        const scale = Math.min(
          116 / Math.max(bounds.w, 1),
          76 / Math.max(bounds.h, 1),
        );
        miniBounds.current = { x: bounds.x1, y: bounds.y1, scale };
        const project = (point: { x: number; y: number }) => ({
          x: (point.x - bounds.x1) * scale + 8,
          y: (point.y - bounds.y1) * scale + 8,
        });
        ctx.strokeStyle = "#355e52";
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
            ctx.fillStyle = n.data("color") || "#87bba7";
            ctx.beginPath();
            ctx.arc(p.x, p.y, 1.7, 0, Math.PI * 2);
            ctx.fill();
          });
        const extent = cy.extent();
        const corner = project({ x: extent.x1, y: extent.y1 });
        ctx.strokeStyle = "#b8d7cc";
        ctx.lineWidth = 1;
        ctx.strokeRect(corner.x, corner.y, extent.w * scale, extent.h * scale);
      });
    };
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
      // Perspectives change the canvas height (statistics and timeline) and width.
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
                idealEdgeLength: () => 150,
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
  useEffect(() => {
    const cy = instance.current;
    if (!cy) return;
    cy.elements().removeClass("is-selected is-connected");
    const selected = cy.getElementById(selectedId);
    selected.addClass("is-selected");
    selected.connectedEdges().addClass("is-connected");
    for (const region of graphRegions(graph, localNode))
      if (
        region.kind === "execution" &&
        resourceKey(region.nodeId, "remote", region.nodeId) === selectedId
      )
        cy.getElementById(region.id).addClass("is-selected");
  }, [selectedId, graph, mode, layout, label, copy, localNode]);
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
    <div className="mesh-canvas-wrap">
      <div
        ref={host}
        className="collab-cytoscape mesh-canvas"
        role="img"
        aria-label={copy.canvas}
      />
      <div
        ref={labelLayer}
        className="mesh-label-layer"
        style={{
          transform: `translate(${camera.x}px, ${camera.y}px) scale(${camera.zoom})`,
        }}
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
          return region.kind === "execution" ? (
            <Button
              variant="outline"
              type="button"
              className="mesh-region-label mesh-region-execution-label"
              data-mesh-region={region.id}
              key={region.id}
              style={{ left: point.x, top: point.y }}
              onClick={() =>
                select(resourceKey(region.nodeId, "remote", region.nodeId))
              }
              title={heading}
            >
              {heading}
            </Button>
          ) : (
            <span
              className="mesh-region-label"
              data-mesh-region={region.id}
              key={region.id}
              style={{ left: point.x, top: point.y }}
              title={heading}
            >
              {heading}
            </span>
          );
        })}
        {graph.nodes.map((node) => {
          const point = camera.points[node.id];
          if (!point) return null;
          return (
            <Button
              variant="outline"
              type="button"
              key={node.id}
              data-mesh-node={node.id}
              className={`mesh-node-label ${selectedId === node.id ? "selected" : ""}`}
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
                left: point.x + labelOffset(node).x,
                top: point.y + labelOffset(node).y,
              }}
              title={`${label(node)} · ${copy.kinds[node.kind]}`}
            >
              <strong>{label(node)}</strong>
              <span className="mesh-node-kind">{copy.kinds[node.kind]}</span>
              {node.status && (
                <span
                  className={`mesh-status status-${node.status.toLowerCase()}`}
                >
                  {status(node)}
                </span>
              )}
              {!node.available && (
                <span className="mesh-node-kind">{copy.missing}</span>
              )}
            </Button>
          );
        })}
      </div>
      <Button
        variant="outline"
        type="button"
        className="mesh-minimap"
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
        <canvas ref={mini} width={132} height={92} />
      </Button>
      <div className="mesh-camera" role="group" aria-label={copy.camera}>
        <Button
          variant="outline"
          type="button"
          aria-label={copy.zoomIn}
          title={copy.zoomIn}
          onClick={() => zoom(1.25)}
        >
          <Plus size={16} />
        </Button>
        <Button
          variant="outline"
          type="button"
          aria-label={copy.zoomOut}
          title={copy.zoomOut}
          onClick={() => zoom(0.8)}
        >
          <Minus size={16} />
        </Button>
        <Button
          variant="outline"
          type="button"
          aria-label={copy.fit}
          className="graph-fit-button"
          title={
            fit.available ? copy.fit : `${copy.fit}: ${copy.fitUnavailable}`
          }
          aria-describedby={fitDescription}
          disabled={!fit.available}
          onClick={fit.request}
        >
          <Scan size={16} aria-hidden="true" />
          <span>{copy.fit}</span>
        </Button>
        <Button
          variant="outline"
          type="button"
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
          <Crosshair size={16} />
        </Button>
        <Button
          variant="outline"
          type="button"
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
          <Maximize size={16} />
        </Button>
      </div>
      <span id={fitDescription} className="graph-fit-status" role="status">
        {fit.available ? "" : copy.fitUnavailable}
      </span>
    </div>
  );
}
