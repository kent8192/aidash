import { useEffect, useRef } from "react";
import cytoscape, { type Core } from "cytoscape";
import type { AgentGraph, GraphNode } from "../agent-graph/model";
import type { GraphCopy } from "../agent-graph/copy";
import { synchronizeGraph } from "./cytoscape-model";

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
  const instance = useRef<Core | null>(null);
  const onSelect = useRef(select);
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
      style: [
        {
          selector: "node",
          style: {
            label: "data(label)",
            "background-color": "#e7f0eb",
            "border-color": "#527462",
            "border-width": 1.5,
            color: "#20372c",
            width: 26,
            height: 26,
            "font-size": 11,
            "text-valign": "bottom",
            "text-margin-y": 8,
            "text-wrap": "ellipsis",
            "text-max-width": "160px",
          },
        },
        {
          selector: "node.root",
          style: {
            "background-color": "#285b44",
            "border-width": 3,
            width: 38,
            height: 38,
          },
        },
        {
          selector: "node.unavailable",
          style: { "background-color": "#d9dde0", "border-style": "dashed" },
        },
        {
          selector: "edge",
          style: {
            width: 1.5,
            "line-color": "#8ca597",
            "target-arrow-color": "#8ca597",
            "target-arrow-shape": "triangle",
            "curve-style": "bezier",
          },
        },
        {
          selector: 'edge[layer = "runtime"]',
          style: {
            "line-style": "dashed",
            "line-color": "#798cab",
            "target-arrow-color": "#798cab",
          },
        },
        {
          selector: ".focused",
          style: { "border-color": "#d3762a", "border-width": 4 },
        },
        { selector: ".dimmed", style: { opacity: 0.28 } },
      ],
    });
    instance.current = cy;
    cy.on("tap", "node", (event) => onSelect.current(event.target.id()));
    const observer = new ResizeObserver(() => cy.resize());
    observer.observe(container.current);
    return () => {
      observer.disconnect();
      cy.destroy();
      instance.current = null;
    };
  }, []);
  useEffect(() => {
    const cy = instance.current;
    if (!cy) return;
    const empty = cy.nodes().empty();
    synchronizeGraph(cy, graph, label);
    // Positions are deterministic; expansions retain both the camera and existing nodes.
    if (empty) cy.fit(undefined, 42);
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
    <div className="agent-graph-canvas">
      <div
        className="agent-graph-controls"
        role="group"
        aria-label={copy.graph}
      >
        <button
          type="button"
          onClick={() => {
            const cy = instance.current;
            if (cy) cy.zoom(Math.min(cy.maxZoom(), cy.zoom() * 1.25));
          }}
          aria-label={copy.zoomIn}
        >
          +
        </button>
        <button
          type="button"
          onClick={() => {
            const cy = instance.current;
            if (cy) cy.zoom(Math.max(cy.minZoom(), cy.zoom() / 1.25));
          }}
          aria-label={copy.zoomOut}
        >
          −
        </button>
        <button
          type="button"
          onClick={() => instance.current?.fit(undefined, 40)}
        >
          {copy.fit}
        </button>
        <button
          type="button"
          onClick={() => {
            const cy = instance.current;
            if (cy) cy.center(cy.getElementById(selectedId));
          }}
        >
          {copy.focus}
        </button>
      </div>
      <div
        ref={container}
        className="collab-cytoscape"
        role="img"
        aria-label={copy.graph}
      />
    </div>
  );
}
