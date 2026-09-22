import { useEffect, useId, useRef, useState } from "react";
import {
  clampZoom,
  layoutGraph,
  relationshipCurve,
  type AgentGraph,
  type GraphNode,
  type Point,
  type Positions,
} from "./model";
import type { GraphCopy } from "./copy";

type Camera = Point & { zoom: number };
function localPoint(
  element: SVGSVGElement,
  x: number,
  y: number,
): Point | null {
  const matrix = element.getScreenCTM();
  if (!matrix) return null;
  const point = new DOMPoint(x, y).matrixTransform(matrix.inverse());
  return Number.isFinite(point.x) && Number.isFinite(point.y)
    ? { x: point.x, y: point.y }
    : null;
}
const width = 800;
const height = 460;
function fit(graph: AgentGraph, positions: Positions): Camera {
  const points = graph.nodes.map((node) => positions[node.id]);
  const minX = Math.min(...points.map((p) => p.x)) - 105;
  const maxX = Math.max(...points.map((p) => p.x)) + 105;
  const minY = Math.min(...points.map((p) => p.y)) - 70;
  const maxY = Math.max(...points.map((p) => p.y)) + 70;
  const zoom = clampZoom(
    Math.min(1, (width - 40) / (maxX - minX), (height - 40) / (maxY - minY)),
  );
  return {
    x: width / 2 - ((minX + maxX) / 2) * zoom,
    y: height / 2 - ((minY + maxY) / 2) * zoom,
    zoom,
  };
}
function zoomAt(camera: Camera, scale: number, point: Point): Camera {
  const zoom = clampZoom(scale);
  const ratio = zoom / camera.zoom;
  return {
    zoom,
    x: point.x - (point.x - camera.x) * ratio,
    y: point.y - (point.y - camera.y) * ratio,
  };
}

export function GraphCanvas({
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
  const signature = JSON.stringify(graph.nodes.map((node) => node.id).sort());
  const [layout, setLayout] = useState(() => ({
    signature,
    positions: layoutGraph(graph.nodes, graph.rootId),
  }));
  let positions = layout.positions;
  // Adjust only when membership changes, not when statuses or pointer positions change.
  if (signature !== layout.signature) {
    positions = layoutGraph(graph.nodes, graph.rootId, layout.positions);
    setLayout({ signature, positions });
  }
  const [camera, setCamera] = useState<Camera>(() => fit(graph, positions));
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{
    pointer: number;
    start: Point;
    camera: Camera;
    node?: string;
    position?: Point;
  } | null>(null);
  const marker = useId().replace(/:/g, "");
  const neighbors = new Set([selectedId]);
  for (const edge of graph.edges) {
    if (edge.source === selectedId) neighbors.add(edge.target);
    if (edge.target === selectedId) neighbors.add(edge.source);
  }
  useEffect(() => {
    const element = svg.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      const point = localPoint(element, event.clientX, event.clientY);
      if (!point) return;
      event.preventDefault();
      setCamera((current) =>
        zoomAt(
          current,
          current.zoom *
            Math.exp(-Math.max(-200, Math.min(200, event.deltaY)) * 0.004),
          point,
        ),
      );
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, []);
  const zoom = (factor: number) =>
    setCamera((current) =>
      zoomAt(current, current.zoom * factor, { x: width / 2, y: height / 2 }),
    );
  const pan = (x: number, y: number) =>
    setCamera((current) => ({
      ...current,
      x: current.x + x,
      y: current.y + y,
    }));
  return (
    <div className="agent-graph-canvas">
      <div
        className="agent-graph-controls"
        role="group"
        aria-label={copy.graph}
      >
        <button
          type="button"
          onClick={() => zoom(1.25)}
          aria-label={copy.zoomIn}
        >
          +
        </button>
        <button
          type="button"
          onClick={() => zoom(0.8)}
          aria-label={copy.zoomOut}
        >
          −
        </button>
        <button type="button" onClick={() => pan(60, 0)} aria-label={copy.left}>
          ←
        </button>
        <button
          type="button"
          onClick={() => pan(-60, 0)}
          aria-label={copy.right}
        >
          →
        </button>
        <button type="button" onClick={() => pan(0, 60)} aria-label={copy.up}>
          ↑
        </button>
        <button
          type="button"
          onClick={() => pan(0, -60)}
          aria-label={copy.down}
        >
          ↓
        </button>
        <button type="button" onClick={() => setCamera(fit(graph, positions))}>
          {copy.fit}
        </button>
        <button
          type="button"
          onClick={() => {
            const point = positions[selectedId] ?? positions[graph.rootId];
            setCamera((current) => ({
              ...current,
              x: width / 2 - point.x * current.zoom,
              y: height / 2 - point.y * current.zoom,
            }));
          }}
        >
          {copy.focus}
        </button>
      </div>
      <p className="muted">{copy.canvasHelp}</p>
      {/* The equivalent native-button relationship list is the keyboard/screen-reader interface. */}
      <svg
        ref={svg}
        viewBox={`0 0 ${width} ${height}`}
        aria-hidden="true"
        focusable="false"
        onPointerDown={(event) => {
          if (event.button !== 0 || drag.current) return;
          const start = localPoint(
            event.currentTarget,
            event.clientX,
            event.clientY,
          );
          if (!start) return;
          const target =
            event.target instanceof Element
              ? event.target.closest<SVGGElement>("[data-graph-node]")
              : null;
          const id = target?.dataset.graphNode;
          if (id) select(id);
          drag.current = {
            pointer: event.pointerId,
            start,
            camera,
            node: id !== graph.rootId ? id : undefined,
            position: id ? positions[id] : undefined,
          };
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerMove={(event) => {
          const current = drag.current;
          if (!current || current.pointer !== event.pointerId) return;
          const point = localPoint(
            event.currentTarget,
            event.clientX,
            event.clientY,
          );
          if (!point) return;
          const dx = point.x - current.start.x;
          const dy = point.y - current.start.y;
          if (current.node && current.position) {
            const id = current.node;
            const point = {
              x: current.position.x + dx / current.camera.zoom,
              y: current.position.y + dy / current.camera.zoom,
            };
            setLayout((value) => ({
              ...value,
              positions: { ...value.positions, [id]: point },
            }));
          } else
            setCamera({
              ...current.camera,
              x: current.camera.x + dx,
              y: current.camera.y + dy,
            });
        }}
        onPointerUp={(event) => {
          if (drag.current?.pointer !== event.pointerId) return;
          drag.current = null;
          if (event.currentTarget.hasPointerCapture(event.pointerId))
            event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={() => {
          drag.current = null;
        }}
        onLostPointerCapture={() => {
          drag.current = null;
        }}
      >
        <defs>
          <marker
            id={marker}
            markerWidth="7"
            markerHeight="7"
            refX="6"
            refY="3.5"
            orient="auto"
          >
            <path d="M0 0 L7 3.5 L0 7" fill="context-stroke" />
          </marker>
        </defs>
        <g
          data-camera="true"
          transform={`translate(${camera.x} ${camera.y}) scale(${camera.zoom})`}
        >
          {graph.edges.map((edge) => {
            const from = positions[edge.source];
            const to = positions[edge.target];
            const curve = relationshipCurve(
              from,
              to,
              edge.source === graph.rootId ? 29 : 21,
              edge.target === graph.rootId ? 31 : 23,
              graph.edges.some(
                (other) =>
                  other.source === edge.target && other.target === edge.source,
              ),
            );
            return (
              <g
                key={edge.id}
                className="agent-graph-edge"
                data-layer={edge.layer}
                data-muted={
                  !(edge.source === selectedId || edge.target === selectedId)
                }
              >
                <path
                  d={curve.path}
                  fill="none"
                  markerEnd={`url(#${marker})`}
                />
                <text
                  x={curve.label.x}
                  y={curve.label.y - 7}
                  textAnchor="middle"
                >
                  {copy.relations[edge.relation]}
                </text>
              </g>
            );
          })}
          {graph.nodes.map((node) => {
            const point = positions[node.id];
            const name = label(node);
            return (
              <g
                key={node.id}
                data-graph-node={node.id}
                data-kind={node.kind}
                data-selected={node.id === selectedId}
                data-unavailable={!node.available}
                data-muted={!neighbors.has(node.id)}
                className="agent-graph-node"
                transform={`translate(${point.x} ${point.y})`}
              >
                <title>
                  {name}
                  {node.status ? ` · ${node.status}` : ""}
                  {node.available ? "" : ` · ${copy.missing}`}
                </title>
                <circle r={node.id === graph.rootId ? 26 : 18} />
                <text className="agent-graph-glyph" textAnchor="middle" y="5">
                  {copy.types[node.kind].slice(0, 1)}
                </text>
                <text className="agent-graph-name" textAnchor="middle" y="45">
                  {name.length > 30 ? `${name.slice(0, 29)}…` : name}
                </text>
                <text className="agent-graph-kind" textAnchor="middle" y="61">
                  {copy.types[node.kind]}
                  {node.status ? ` · ${node.status}` : ""}
                </text>
              </g>
            );
          })}
        </g>
      </svg>
    </div>
  );
}
