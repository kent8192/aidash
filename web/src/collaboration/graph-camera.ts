import type { Core } from "cytoscape";
import {
  fitGraph,
  unionBounds,
  zoomFloor,
  type Bounds,
  type GraphCamera,
} from "../graph-fit";
import { graphViewport } from "../graph-fit-view";

export function cytoscapeFit(
  cy: Core,
  host: HTMLElement,
  originalFloor: number,
  labels: Bounds[] = [],
): GraphCamera | null {
  const style = getComputedStyle(host);
  const width =
    host.clientWidth -
    parseFloat(style.paddingLeft) -
    parseFloat(style.paddingRight);
  const height =
    host.clientHeight -
    parseFloat(style.paddingTop) -
    parseFloat(style.paddingBottom);
  if (
    Math.abs(cy.width() - width) > 0.1 ||
    Math.abs(cy.height() - height) > 0.1
  )
    cy.resize();
  if (cy.nodes().empty()) return null;
  if (
    cy
      .nodes()
      .map((node) => node.position())
      .some((point) => !Object.values(point).every(Number.isFinite))
  )
    return null;
  const scope = host.closest(".mesh-graph");
  const blockers = scope
    ? [
        ...scope.querySelectorAll(
          ".mesh-inspector, .mesh-minimap, .mesh-camera, .mesh-filters",
        ),
      ]
    : [];
  const viewport = graphViewport(host, blockers);
  if (!viewport) return null;
  const rect = host.getBoundingClientRect();
  const camera = fitGraph(
    unionBounds([cy.elements().boundingBox(), ...labels]),
    {
      x1: viewport.x1 - rect.left,
      y1: viewport.y1 - rect.top,
      x2: viewport.x2 - rect.left,
      y2: viewport.y2 - rect.top,
    },
  );
  cy.minZoom(zoomFloor(originalFloor, camera?.zoom, cy.zoom()));
  return camera;
}
