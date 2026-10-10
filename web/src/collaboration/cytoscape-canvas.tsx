import {
  graphTokens,
  observeGraphTheme,
  type GraphTokens,
} from "./graph-theme";
import { Button } from "../components/ui/button";
import { useEffect, useId, useRef } from "react";
import cytoscape, { type Core, type StylesheetJson } from "cytoscape";
import type { AgentGraph, GraphNode } from "../agent-graph/model";
import type { GraphCopy } from "../agent-graph/copy";
import { Crosshair, Minus, Plus, Scan } from "lucide-react";
import { useGraphFit } from "../graph-fit-view";
import { cytoscapeFit } from "./graph-camera";
import { synchronizeGraph } from "./cytoscape-model";

function graphStylesheet(t: GraphTokens): StylesheetJson {
  return [
    {
      selector: "node",
      style: {
        label: "data(label)",
        shape: "round-rectangle",
        "corner-radius": "10",
        width: 30,
        height: 30,
        "background-color": t.surface,
        "border-color": t.borderStrong,
        "border-width": 1,
        color: t.foreground,
        "font-family": t.sans,
        "font-size": 11,
        "text-valign": "bottom",
        "text-margin-y": 8,
        "text-wrap": "ellipsis",
        "text-max-width": "160px",
        "text-outline-color": t.background,
        "text-outline-width": 2,
      },
    },
    {
      selector: "node.root",
      style: {
        "background-color": t.raised,
        "border-color": t.brandMark,
        "border-width": 2,
        width: 38,
        height: 38,
      },
    },
    {
      selector: "node.unavailable",
      style: {
        "background-color": t.background,
        "border-color": t.faint,
        "border-style": "dashed",
        color: t.faint,
      },
    },
    {
      selector: "edge",
      style: {
        width: 1,
        "line-color": t.edge,
        "target-arrow-color": t.edge,
        "target-arrow-shape": "triangle",
        "arrow-scale": 0.8,
        "curve-style": "bezier",
      },
    },
    {
      selector: 'edge[layer = "runtime"]',
      style: {
        "line-style": "dashed",
        "line-color": t.edgeStrong,
        "target-arrow-color": t.edgeStrong,
      },
    },
    {
      selector: "node.focused",
      style: {
        "outline-color": t.brandMark,
        "outline-width": 2,
        "outline-offset": 3,
        "underlay-color": t.brandMark,
        "underlay-opacity": 0.15,
        "underlay-padding": 8,
      },
    },
    { selector: ".dimmed", style: { opacity: 0.3 } },
  ];
}

export function CytoscapeCanvas({
  graph,
  selectedId,
  label,
  select,
  copy,
}: {
  graph: AgentGraph;
  selectedId: string;
  label: (node: GraphNode) => string;
  select: (id: string) => void;
  copy: GraphCopy;
}) {
  const container = useRef<HTMLDivElement>(null);
  const needsFraming = useRef(true);
  const root = useRef(graph.rootId);
  const fitDescription = useId();
  const instance = useRef<Core | null>(null);
  const onSelect = useRef(select);
  const fit = useGraphFit(
    container,
    () => {
      const cy = instance.current;
      return cy && container.current && cy.nodes().length === graph.nodes.length
        ? cytoscapeFit(cy, container.current, 0.15)
        : null;
    },
    (camera) => {
      needsFraming.current = false;
      instance.current?.viewport({
        zoom: camera.zoom,
        pan: { x: camera.x, y: camera.y },
      });
    },
    graph.rootId,
  );
  useEffect(() => {
    if (needsFraming.current && fit.available && graph.nodes.length)
      fit.request();
  });
  useEffect(() => {
    onSelect.current = select;
  }, [select]);
  useEffect(() => {
    if (!container.current) return;
    const cy = cytoscape({
      container: container.current,
      elements: [],
      minZoom: 0.15,
      maxZoom: 4,
      wheelSensitivity: 0.2,
      boxSelectionEnabled: false,
      style: graphStylesheet(graphTokens()),
    });
    instance.current = cy;
    const stopTheme = observeGraphTheme(cy, graphStylesheet);
    cy.on("tap", "node", (event) => onSelect.current(event.target.id()));
    const observer = new ResizeObserver(() => cy.resize());
    observer.observe(container.current);
    return () => {
      observer.disconnect();
      stopTheme();
      cy.destroy();
      instance.current = null;
    };
  }, []);
  useEffect(() => {
    const cy = instance.current;
    if (!cy) return;
    if (root.current !== graph.rootId) {
      root.current = graph.rootId;
      needsFraming.current = true;
    }
    synchronizeGraph(cy, graph, label);
    // Positions are deterministic; expansions retain both the camera and existing nodes.
  }, [graph, label]);
  useEffect(() => {
    const cy = instance.current;
    if (!cy) return;
    cy.elements().removeClass("focused dimmed");
    const selected = cy.getElementById(selectedId);
    if (!selected.empty()) {
      selected.addClass("focused");
      cy.elements()
        .difference(selected.closedNeighborhood())
        .addClass("dimmed");
    }
  }, [selectedId, graph]);
  return (
    <div className="agent-graph-canvas relative h-full min-h-[320px] w-full">
      <div
        className="absolute right-3 top-3 z-[1] inline-flex items-center gap-0.5 rounded-md border border-border-strong bg-surface/90 p-0.5 shadow-overlay"
        role="group"
        aria-label={copy.graph}
      >
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          type="button"
          onClick={() => {
            fit.cancel();
            needsFraming.current = false;
            const cy = instance.current;
            if (cy) cy.zoom(Math.min(cy.maxZoom(), cy.zoom() * 1.25));
          }}
          aria-label={copy.zoomIn}
        >
          <Plus size={14} aria-hidden="true" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          type="button"
          onClick={() => {
            fit.cancel();
            needsFraming.current = false;
            const cy = instance.current;
            if (cy) cy.zoom(Math.max(cy.minZoom(), cy.zoom() / 1.25));
          }}
          aria-label={copy.zoomOut}
        >
          <Minus size={14} aria-hidden="true" />
        </Button>
        <span aria-hidden className="mx-0.5 h-4 w-px bg-border" />
        <Button
          variant="ghost"
          size="sm"
          type="button"
          aria-label={copy.fit}
          title={
            fit.available ? copy.fit : `${copy.fit}: ${copy.fitUnavailable}`
          }
          aria-describedby={fitDescription}
          disabled={!fit.available}
          onClick={fit.request}
        >
          <Scan size={14} aria-hidden="true" />
          <span>{copy.fit}</span>
        </Button>
        <Button
          variant="ghost"
          size="sm"
          type="button"
          onClick={() => {
            fit.cancel();
            needsFraming.current = false;
            const cy = instance.current;
            if (cy) cy.center(cy.getElementById(selectedId));
          }}
        >
          <Crosshair size={14} aria-hidden="true" />
          {copy.focus}
        </Button>
      </div>
      <span id={fitDescription} className="sr-only" role="status">
        {fit.available ? "" : copy.fitUnavailable}
      </span>
      <div
        ref={container}
        className="collab-cytoscape h-full min-h-[320px] w-full touch-none"
        role="img"
        aria-label={copy.graph}
      />
    </div>
  );
}
