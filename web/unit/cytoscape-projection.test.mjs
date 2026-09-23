import assert from "node:assert/strict";
import { test } from "node:test";
import cytoscape from "cytoscape";
import {
  toElements,
  synchronizeGraph,
} from "../src/collaboration/cytoscape-model.ts";
const graph = {
  rootId: '["agent","one"]',
  nodes: [
    {
      id: '["agent","one"]',
      kind: "agent",
      name: { en: "One" },
      available: true,
      entity: { id: "one", version: "1" },
    },
    {
      id: "model.v1",
      kind: "model",
      name: { en: "Model" },
      available: true,
      entity: { id: "model", version: "1" },
    },
  ],
  edges: [
    {
      id: "edge.[one]",
      source: '["agent","one"]',
      target: "model.v1",
      relation: "model",
      layer: "configuration",
    },
  ],
  omitted: 0,
  omittedEdges: 0,
};
const label = (node) => node.name.en;
test("Cytoscape projection preserves opaque exact identifiers and relationship type", () => {
  const elements = toElements(graph, label);
  assert.equal(elements.length, 3);
  const cy = cytoscape({ headless: true, elements });
  assert.equal(cy.nodes().length, 2);
  assert.equal(cy.edges()[0].data("layer"), "configuration");
  assert.equal(cy.edges()[0].source().id(), graph.rootId);
  cy.destroy();
});
test("model projection does not copy private or arbitrary input fields", () => {
  const elements = toElements(
    {
      ...graph,
      nodes: [
        {
          ...graph.nodes[0],
          private: "do-not-render",
          config: { secret: "secret" },
        },
      ],
    },
    label,
  );
  assert.ok(!JSON.stringify(elements).includes("do-not-render"));
  assert.ok(!JSON.stringify(elements).includes("secret"));
});
test("refresh removes revoked nodes and incident edges without resetting retained positions", () => {
  const cy = cytoscape({
    headless: true,
    elements: toElements(graph, label),
    layout: { name: "preset" },
  });
  cy.getElementById(graph.rootId).position({ x: 50, y: 90 });
  synchronizeGraph(cy, { ...graph, nodes: [graph.nodes[0]], edges: [] }, label);
  assert.equal(cy.nodes().length, 1);
  assert.equal(cy.edges().length, 0);
  assert.deepEqual(cy.getElementById(graph.rootId).position(), {
    x: 50,
    y: 90,
  });
  cy.destroy();
});
test("status refresh updates the retained node instead of duplicating it", () => {
  const cy = cytoscape({ headless: true, elements: toElements(graph, label) });
  synchronizeGraph(
    cy,
    {
      ...graph,
      nodes: graph.nodes.map((node) => ({ ...node, status: "WAITING" })),
    },
    label,
  );
  assert.equal(cy.nodes().length, 2);
  assert.equal(cy.getElementById(graph.rootId).data("status"), "WAITING");
  cy.destroy();
});
