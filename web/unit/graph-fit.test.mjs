import assert from "node:assert/strict";
import { test } from "node:test";
import {
  fitGraph,
  fitPadding,
  unionBounds,
  unobscuredViewport,
  zoomFloor,
} from "../src/graph-fit.ts";

const viewport = { x1: 25, y1: 40, x2: 925, y2: 740 };
for (const [name, bounds] of [
  [
    "disconnected components",
    [
      { x1: -1400, y1: -900, x2: -1200, y2: -800 },
      { x1: 800, y1: 1700, x2: 1100, y2: 1800 },
    ],
  ],
  ["wide graph", [{ x1: -1e6, y1: 0, x2: 1e6, y2: 60 }]],
  ["tall graph", [{ x1: 0, y1: -1e6, x2: 60, y2: 1e6 }]],
  ["single node", [{ x1: -24, y1: -25, x2: 24, y2: 90 }]],
  [
    "labels and groups",
    [
      { x1: -650, y1: -120, x2: 700, y2: 350 },
      { x1: -900, y1: 300, x2: -700, y2: 450 },
    ],
  ],
]) {
  test(`fit contains and centers ${name} without excessive magnification`, () => {
    const union = unionBounds(bounds);
    const result = fitGraph(union, viewport);
    assert.ok(result && result.zoom > 0 && result.zoom <= 1);
    const padding = 36;
    for (const box of bounds) {
      assert.ok(
        box.x1 * result.zoom + result.x >= viewport.x1 + padding - 1e-8,
      );
      assert.ok(
        box.x2 * result.zoom + result.x <= viewport.x2 - padding + 1e-8,
      );
      assert.ok(
        box.y1 * result.zoom + result.y >= viewport.y1 + padding - 1e-8,
      );
      assert.ok(
        box.y2 * result.zoom + result.y <= viewport.y2 - padding + 1e-8,
      );
    }
    assert.ok(
      Math.abs(((union.x1 + union.x2) / 2) * result.zoom + result.x - 475) <
        1e-8,
    );
    assert.ok(
      Math.abs(((union.y1 + union.y2) / 2) * result.zoom + result.y - 390) <
        1e-8,
    );
    assert.deepEqual(fitGraph(union, viewport), result);
    if (name.includes("wide") || name.includes("tall"))
      assert.ok(result.zoom < 0.12);
  });
}

test("empty, invalid or unavailable geometry does not yield a partial camera", () => {
  assert.equal(fitGraph(unionBounds([]), viewport), null);
  assert.equal(
    fitGraph(
      unionBounds([
        { x1: 0, y1: 0, x2: 100, y2: 50 },
        { x1: NaN, y1: 0, x2: 10, y2: 10 },
      ]),
      viewport,
    ),
    null,
  );
  assert.equal(
    fitGraph(
      { x1: 0, y1: 0, x2: 100, y2: 50 },
      { x1: 5, y1: 0, x2: 5, y2: 50 },
    ),
    null,
  );
});

test("compact and extremely small positive viewports keep a positive interior", () => {
  for (const size of [300, 40, 0.5]) {
    const viewport = { x1: 0, y1: 0, x2: size, y2: size };
    assert.equal(fitPadding(viewport), size < 64 ? size / 4 : 16);
    const camera = fitGraph({ x1: 0, y1: 0, x2: 100, y2: 100 }, viewport);
    assert.ok(camera && Number.isFinite(camera.zoom) && camera.zoom > 0);
  }
});

test("panels and controls leave the largest deterministic usable rectangle", () => {
  const canvas = { x1: 0, y1: 0, x2: 1000, y2: 800 };
  const blockers = [
    { x1: 750, y1: 0, x2: 1000, y2: 800 },
    { x1: 0, y1: 500, x2: 750, y2: 800 },
  ];
  assert.deepEqual(unobscuredViewport(canvas, blockers), {
    x1: 0,
    y1: 0,
    x2: 750,
    y2: 500,
  });
  assert.equal(unobscuredViewport(canvas, [canvas]), null);
  assert.deepEqual(
    unobscuredViewport(canvas, [{ x1: 1500, y1: 0, x2: 2000, y2: 800 }]),
    canvas,
  );
});

test("zoom limit updates permit an overview and preserve a lower current camera", () => {
  assert.ok(zoomFloor(0.12, 0.003, 0.5) > 0);
  assert.ok(zoomFloor(0.12, 0.003, 0.5) < 0.003);
  assert.equal(zoomFloor(0.12, 0.4, 0.003), 0.003);
  assert.equal(zoomFloor(0.12, undefined, 0.05), 0.05);
});
