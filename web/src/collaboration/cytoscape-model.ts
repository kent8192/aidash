import type { Core, ElementDefinition } from "cytoscape";
import {
  layoutGraph,
  type AgentGraph,
  type GraphNode,
} from "../agent-graph/model.ts";

/** Whitelist display fields; do not send entity configuration to the renderer. */
export function toElements(
  graph: AgentGraph,
  label: (node: GraphNode) => string,
): ElementDefinition[] {
  const positions = layoutGraph(graph.nodes, graph.rootId);
  return [
    ...graph.nodes.map((node) => ({
      group: "nodes" as const,
      position: positions[node.id],
      data: {
        id: node.id,
        label: label(node),
        kind: node.kind,
        available: node.available,
        status: node.status ?? "",
      },
      classes: [
        node.id === graph.rootId ? "root" : "",
        node.available ? "" : "unavailable",
      ]
        .filter(Boolean)
        .join(" "),
    })),
    ...graph.edges.map((edge) => ({
      group: "edges" as const,
      data: {
        id: edge.id,
        source: edge.source,
        target: edge.target,
        label: edge.relation,
        layer: edge.layer,
      },
    })),
  ];
}

/** Update exact IDs without discarding a user's node positions and camera. */
export function synchronizeGraph(
  cy: Core,
  graph: AgentGraph,
  label: (node: GraphNode) => string,
): boolean {
  const elements = toElements(graph, label);
  const wanted = new Set(elements.map((element) => element.data.id));
  let membershipChanged = false;
  cy.batch(() => {
    cy.elements().forEach((element) => {
      if (!wanted.has(element.id())) {
        element.remove();
        membershipChanged = true;
      }
    });
    for (const definition of elements) {
      const element = cy.getElementById(definition.data.id!);
      if (element.empty()) {
        cy.add(definition);
        membershipChanged = true;
      } else {
        element.data(definition.data);
        element.classes(definition.classes ?? "");
      }
    }
  });
  return membershipChanged;
}
