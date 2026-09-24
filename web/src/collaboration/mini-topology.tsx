import { useEffect, useRef } from "react";
import cytoscape from "cytoscape";
import type { Task } from "../types";
import { channelAgents, type LocatedRun } from "./workspace-model";

export default function MiniTopology({
  runs,
  tasks,
  label,
  title,
}: {
  runs: LocatedRun[];
  tasks: Task[];
  label: (item: LocatedRun) => string;
  title: string;
}) {
  const container = useRef<HTMLDivElement>(null);
  // The miniature is an actual projection of task ownership/dependencies, never inferred collaboration.
  const elements = JSON.stringify({
    nodes: channelAgents(runs).map((item) => ({
      data: {
        id: JSON.stringify([
          item.node,
          item.run.agent_id,
          item.run.agent_version,
        ]),
        label: label(item),
        remote: item.node !== item.run.home_node,
      },
    })),
    edges: tasks.flatMap((task) =>
      task.dependencies.flatMap((dependency) => {
        const ordered = [...runs].sort((a, b) =>
          b.run.updated_at.localeCompare(a.run.updated_at),
        );
        const source = ordered.find((item) => item.run.task_id === dependency);
        const target = ordered.find((item) => item.run.task_id === task.id);
        if (!source || !target) return [];
        const from = JSON.stringify([
          source.node,
          source.run.agent_id,
          source.run.agent_version,
        ]);
        const to = JSON.stringify([
          target.node,
          target.run.agent_id,
          target.run.agent_version,
        ]);
        return from === to
          ? []
          : [
              {
                data: {
                  id: `${dependency}:${task.id}`,
                  source: from,
                  target: to,
                },
              },
            ];
      }),
    ),
  });
  useEffect(() => {
    if (!container.current) return;
    const cy = cytoscape({
      container: container.current,
      elements: JSON.parse(elements),
      layout: { name: "circle", fit: false, radius: 32 },
      userZoomingEnabled: false,
      userPanningEnabled: false,
      autoungrabify: true,
      style: [
        {
          selector: "node",
          style: {
            "background-color": "#dce8d6",
            "border-color": "#bed0bb",
            "border-width": 1,
            width: 23,
            height: 23,
            label: "data(label)",
            "font-size": 8,
            color: "#61765f",
            "text-valign": "bottom",
            "text-margin-y": 5,
            "text-wrap": "ellipsis",
            "text-max-width": "65px",
          },
        },
        {
          selector: "node[?remote]",
          style: { "background-color": "#e8e1ef", "border-color": "#d4c8df" },
        },
        {
          selector: "edge",
          style: {
            width: 1,
            "line-color": "#a7baa0",
            "target-arrow-color": "#a7baa0",
            "target-arrow-shape": "triangle",
            "curve-style": "bezier",
          },
        },
      ],
    });
    const observer = new ResizeObserver(() => {
      cy.resize();
      cy.zoom(1);
      cy.center();
    });
    observer.observe(container.current);
    const updateColors = () => {
      const color = getComputedStyle(container.current!)
        .getPropertyValue("--ws-muted")
        .trim();
      cy.style().selector("node").style("color", color).update();
    };
    updateColors();
    const themeObserver = new MutationObserver(updateColors);
    const app = container.current.closest(".collab-app");
    if (app)
      themeObserver.observe(app, {
        attributes: true,
        attributeFilter: ["data-theme"],
      });
    return () => {
      observer.disconnect();
      themeObserver.disconnect();
      cy.destroy();
    };
  }, [elements]);
  return (
    <div
      ref={container}
      className="workspace-mini-graph"
      role="img"
      aria-label={title}
    />
  );
}
