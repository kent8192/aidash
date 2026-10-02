export type Bounds = { x1: number; y1: number; x2: number; y2: number };
export type GraphCamera = { x: number; y: number; zoom: number };

export function validBounds(bounds: Bounds): boolean {
  return (
    Object.values(bounds).every(Number.isFinite) &&
    bounds.x2 >= bounds.x1 &&
    bounds.y2 >= bounds.y1
  );
}

/** Invalid members invalidate the entire fit instead of disappearing from it. */
export function unionBounds(bounds: Bounds[]): Bounds | null {
  if (!bounds.length || bounds.some((box) => !validBounds(box))) return null;
  return {
    x1: Math.min(...bounds.map((box) => box.x1)),
    y1: Math.min(...bounds.map((box) => box.y1)),
    x2: Math.max(...bounds.map((box) => box.x2)),
    y2: Math.max(...bounds.map((box) => box.y2)),
  };
}

export function fitPadding(viewport: Bounds): number {
  const shorter = Math.min(
    viewport.x2 - viewport.x1,
    viewport.y2 - viewport.y1,
  );
  return Math.min(shorter / 4, shorter < 360 ? 16 : 36);
}

/** Viewport and padding use the renderer's camera units; zoom never exceeds 1. */
export function fitGraph(
  bounds: Bounds | null,
  viewport: Bounds,
  padding = { x: fitPadding(viewport), y: fitPadding(viewport) },
): GraphCamera | null {
  if (!bounds || !validBounds(bounds) || !validBounds(viewport)) return null;
  const width = viewport.x2 - viewport.x1 - 2 * padding.x;
  const height = viewport.y2 - viewport.y1 - 2 * padding.y;
  if (
    !(width > 0 && height > 0) ||
    !Object.values(padding).every(
      (value) => Number.isFinite(value) && value >= 0,
    )
  )
    return null;
  const zoom = Math.min(
    1,
    width / (bounds.x2 - bounds.x1 || 1),
    height / (bounds.y2 - bounds.y1 || 1),
  );
  const camera = {
    zoom,
    x:
      viewport.x1 +
      (viewport.x2 - viewport.x1) / 2 -
      (bounds.x1 + (bounds.x2 - bounds.x1) / 2) * zoom,
    y:
      viewport.y1 +
      (viewport.y2 - viewport.y1) / 2 -
      (bounds.y1 + (bounds.y2 - bounds.y1) / 2) * zoom,
  };
  return zoom > 0 && Object.values(camera).every(Number.isFinite)
    ? camera
    : null;
}

export function zoomFloor(
  original: number,
  fitted: number | undefined,
  current: number,
): number {
  // Keep one octave below the overview available for gradual manual zoom-out.
  return Math.min(
    original,
    fitted === undefined ? original : Math.max(Number.MIN_VALUE, fitted / 2),
    current,
  );
}

/** Largest empty axis-aligned rectangle, with stable top/left tie breaking. */
export function unobscuredViewport(
  canvas: Bounds,
  obstacles: Bounds[],
): Bounds | null {
  if (!validBounds(canvas) || obstacles.some((box) => !validBounds(box)))
    return null;
  const blocked = obstacles
    .map((box) => ({
      x1: Math.max(canvas.x1, box.x1),
      y1: Math.max(canvas.y1, box.y1),
      x2: Math.min(canvas.x2, box.x2),
      y2: Math.min(canvas.y2, box.y2),
    }))
    .filter((box) => box.x2 > box.x1 && box.y2 > box.y1);
  const xs = [
    ...new Set([canvas.x1, canvas.x2, ...blocked.flatMap((b) => [b.x1, b.x2])]),
  ].sort((a, b) => a - b);
  const ys = [
    ...new Set([canvas.y1, canvas.y2, ...blocked.flatMap((b) => [b.y1, b.y2])]),
  ].sort((a, b) => a - b);
  let best: Bounds | null = null;
  let area = 0;
  for (let top = 0; top < ys.length - 1; top++)
    for (let left = 0; left < xs.length - 1; left++)
      for (let bottom = top + 1; bottom < ys.length; bottom++)
        for (let right = left + 1; right < xs.length; right++) {
          const box = {
            x1: xs[left],
            y1: ys[top],
            x2: xs[right],
            y2: ys[bottom],
          };
          const size = (box.x2 - box.x1) * (box.y2 - box.y1);
          if (
            size > area &&
            !blocked.some(
              (b) =>
                box.x1 < b.x2 &&
                box.x2 > b.x1 &&
                box.y1 < b.y2 &&
                box.y2 > b.y1,
            )
          ) {
            best = box;
            area = size;
          }
        }
  return best;
}
