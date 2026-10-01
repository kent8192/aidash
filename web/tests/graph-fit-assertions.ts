import { expect, type Locator } from "@playwright/test";
import type { Core } from "cytoscape";

export async function cytoscapeCamera(canvas: Locator) {
  return canvas.evaluate((element) => {
    const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg.cy;
    return {
      zoom: cy.zoom(),
      pan: cy.pan(),
      nodes: cy
        .nodes()
        .map((node) => ({ id: node.id(), position: node.position() })),
    };
  });
}

/** Inspect actual rendered extents, independently of the application's fit helper. */
export async function expectCytoscapeFitted(canvas: Locator) {
  await expect
    .poll(async () =>
      canvas.evaluate((element) => {
        const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg
          .cy;
        const host = element.getBoundingClientRect();
        const box = cy.elements().renderedBoundingBox();
        const labels = [
          ...element.parentElement!.querySelectorAll(".mesh-node-label"),
        ].map((label) => label.getBoundingClientRect());
        const visuals = [
          {
            left: host.left + box.x1,
            right: host.left + box.x2,
            top: host.top + box.y1,
            bottom: host.top + box.y2,
          },
          ...labels,
        ];
        const obstacles = [
          ...(element
            .closest(".mesh-graph")
            ?.querySelectorAll(
              ".mesh-camera, .mesh-minimap, .mesh-inspector, .mesh-filters",
            ) ?? []),
        ]
          .filter((obstacle) => obstacle.getClientRects().length)
          .map((obstacle) => obstacle.getBoundingClientRect());
        const errors: string[] = [];
        // The independent oracle enumerates rectangles defined by actual UI edges.
        const visible = {
          left: Math.max(0, host.left),
          top: Math.max(0, host.top),
          right: Math.min(innerWidth, host.right),
          bottom: Math.min(innerHeight, host.bottom),
        };
        for (
          let parent = element.parentElement;
          parent;
          parent = parent.parentElement
        ) {
          const style = getComputedStyle(parent),
            rect = parent.getBoundingClientRect();
          if (/(auto|hidden|scroll|clip)/.test(style.overflowX)) {
            visible.left = Math.max(
              visible.left,
              rect.left + parent.clientLeft,
            );
            visible.right = Math.min(
              visible.right,
              rect.left + parent.clientLeft + parent.clientWidth,
            );
          }
          if (/(auto|hidden|scroll|clip)/.test(style.overflowY)) {
            visible.top = Math.max(visible.top, rect.top + parent.clientTop);
            visible.bottom = Math.min(
              visible.bottom,
              rect.top + parent.clientTop + parent.clientHeight,
            );
          }
        }
        const xs = [
          visible.left,
          visible.right,
          ...obstacles.flatMap((rect) => [
            Math.max(visible.left, Math.min(visible.right, rect.left)),
            Math.max(visible.left, Math.min(visible.right, rect.right)),
          ]),
        ];
        const ys = [
          visible.top,
          visible.bottom,
          ...obstacles.flatMap((rect) => [
            Math.max(visible.top, Math.min(visible.bottom, rect.top)),
            Math.max(visible.top, Math.min(visible.bottom, rect.bottom)),
          ]),
        ];
        const intervals = (values: number[]) =>
          values.flatMap((start) =>
            values.filter((end) => end > start).map((end) => [start, end]),
          );
        const candidates = intervals(xs)
          .flatMap(([left, right]) =>
            intervals(ys).map(([top, bottom]) => ({
              left,
              right,
              top,
              bottom,
            })),
          )
          .filter(
            (rect) =>
              !obstacles.some(
                (blocker) =>
                  rect.left < blocker.right &&
                  rect.right > blocker.left &&
                  rect.top < blocker.bottom &&
                  rect.bottom > blocker.top,
              ),
          )
          .sort(
            (a, b) =>
              (b.right - b.left) * (b.bottom - b.top) -
                (a.right - a.left) * (a.bottom - a.top) ||
              a.top - b.top ||
              a.left - b.left ||
              a.bottom - b.bottom ||
              a.right - b.right,
          );
        const usable = candidates[0];
        if (usable) {
          const shorter = Math.min(
            usable.right - usable.left,
            usable.bottom - usable.top,
          );
          const padding = Math.min(shorter / 4, shorter < 360 ? 16 : 36);
          const outside = visuals.filter(
            (visual) =>
              visual.left < usable.left + padding - 1 ||
              visual.right > usable.right - padding + 1 ||
              visual.top < usable.top + padding - 1 ||
              visual.bottom > usable.bottom - padding + 1,
          );
          if (outside.length)
            errors.push(
              `outside the padded unobscured viewport: ${JSON.stringify({ usable, padding, outside })}`,
            );
        }
        const x =
          (Math.min(...visuals.map((box) => box.left)) +
            Math.max(...visuals.map((box) => box.right))) /
          2;
        const y =
          (Math.min(...visuals.map((box) => box.top)) +
            Math.max(...visuals.map((box) => box.bottom))) /
          2;
        if (
          !usable ||
          Math.abs(x - (usable.left + usable.right) / 2) > 1 ||
          Math.abs(y - (usable.top + usable.bottom) / 2) > 1
        )
          errors.push("visual bounds are not centered in the usable viewport");
        return [...new Set(errors)];
      }),
    )
    .toEqual([]);
}

export async function expectSvgFitted(canvas: Locator) {
  await expect
    .poll(async () =>
      canvas.evaluate((element) => {
        const host = element.getBoundingClientRect();
        let left = Math.max(0, host.left),
          top = Math.max(0, host.top);
        let right = Math.min(innerWidth, host.right),
          bottom = Math.min(innerHeight, host.bottom);
        for (
          let parent = element.parentElement;
          parent;
          parent = parent.parentElement
        ) {
          const style = getComputedStyle(parent);
          const rect = parent.getBoundingClientRect();
          if (/(auto|hidden|scroll|clip)/.test(style.overflowX)) {
            left = Math.max(left, rect.left + parent.clientLeft);
            right = Math.min(
              right,
              rect.left + parent.clientLeft + parent.clientWidth,
            );
          }
          if (/(auto|hidden|scroll|clip)/.test(style.overflowY)) {
            top = Math.max(top, rect.top + parent.clientTop);
            bottom = Math.min(
              bottom,
              rect.top + parent.clientTop + parent.clientHeight,
            );
          }
        }
        const shapes = [
          ...element.querySelectorAll(
            "[data-camera] circle, [data-camera] path, [data-camera] text",
          ),
        ].map((shape) => shape.getBoundingClientRect());
        const bounds = {
          left: Math.min(...shapes.map((shape) => shape.left)),
          right: Math.max(...shapes.map((shape) => shape.right)),
          top: Math.min(...shapes.map((shape) => shape.top)),
          bottom: Math.max(...shapes.map((shape) => shape.bottom)),
        };
        const errors: string[] = [];
        const shorter = Math.min(right - left, bottom - top);
        const padding = Math.min(shorter / 4, shorter < 360 ? 16 : 36);
        if (
          bounds.left < left + padding - 1 ||
          bounds.right > right - padding + 1 ||
          bounds.top < top + padding - 1 ||
          bounds.bottom > bottom - padding + 1
        )
          errors.push(
            `outside visible canvas padding: ${JSON.stringify({ bounds, viewport: { left, top, right, bottom } })}`,
          );
        if (
          Math.abs((bounds.left + bounds.right - left - right) / 2) > 1 ||
          Math.abs((bounds.top + bounds.bottom - top - bottom) / 2) > 1
        )
          errors.push(
            `visual bounds are not centered: ${JSON.stringify({ bounds, viewport: { left, top, right, bottom } })}`,
          );
        return errors;
      }),
    )
    .toEqual([]);
}
