import "./graph-fit.css";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
import {
  unobscuredViewport,
  unionBounds,
  type Bounds,
  type GraphCamera,
} from "./graph-fit";

const rectBounds = (rect: DOMRect): Bounds => ({
  x1: rect.left,
  y1: rect.top,
  x2: rect.right,
  y2: rect.bottom,
});

/** Clip to the screen and scroll containers before subtracting visible panels. */
export function graphViewport(
  element: Element,
  blockers: Element[] = [],
): Bounds | null {
  if (!element.getClientRects().length) return null;
  const rect = rectBounds(element.getBoundingClientRect());
  rect.x1 = Math.max(rect.x1, 0);
  rect.y1 = Math.max(rect.y1, 0);
  rect.x2 = Math.min(rect.x2, window.innerWidth);
  rect.y2 = Math.min(rect.y2, window.innerHeight);
  for (
    let parent = element.parentElement;
    parent;
    parent = parent.parentElement
  ) {
    const style = getComputedStyle(parent);
    const box = parent.getBoundingClientRect();
    if (/(auto|scroll|hidden|clip)/.test(style.overflowX)) {
      rect.x1 = Math.max(rect.x1, box.left + parent.clientLeft);
      rect.x2 = Math.min(
        rect.x2,
        box.left + parent.clientLeft + parent.clientWidth,
      );
    }
    if (/(auto|scroll|hidden|clip)/.test(style.overflowY)) {
      rect.y1 = Math.max(rect.y1, box.top + parent.clientTop);
      rect.y2 = Math.min(
        rect.y2,
        box.top + parent.clientTop + parent.clientHeight,
      );
    }
  }
  return unobscuredViewport(
    rect,
    blockers
      .filter((blocker) => blocker.getClientRects().length)
      .map((blocker) => rectBounds(blocker.getBoundingClientRect())),
  );
}

export function svgBounds(svg: SVGSVGElement): Bounds | null {
  const group = svg.querySelector<SVGGElement>("[data-camera]");
  const matrix = group?.getCTM();
  if (!group || !matrix) return null;
  try {
    const inverse = matrix.inverse();
    const boxes = [
      ...group.querySelectorAll<SVGGraphicsElement>("circle, path, text"),
    ].map((element) => {
      const transform = element.getCTM();
      if (!transform) return { x1: NaN, y1: NaN, x2: NaN, y2: NaN };
      const box = element.getBBox();
      const style = getComputedStyle(element);
      const stroke =
        style.stroke === "none" ? 0 : parseFloat(style.strokeWidth);
      // getBBox stroke/marker options are not consistently implemented by browsers.
      const extra = element.hasAttribute("marker-end")
        ? stroke * 7
        : stroke / 2;
      const model = inverse.multiply(transform);
      const points = [
        [box.x - extra, box.y - extra],
        [box.x + box.width + extra, box.y - extra],
        [box.x - extra, box.y + box.height + extra],
        [box.x + box.width + extra, box.y + box.height + extra],
      ].map(([x, y]) => new DOMPoint(x, y).matrixTransform(model));
      return {
        x1: Math.min(...points.map((point) => point.x)),
        y1: Math.min(...points.map((point) => point.y)),
        x2: Math.max(...points.map((point) => point.x)),
        y2: Math.max(...points.map((point) => point.y)),
      };
    });
    return unionBounds(boxes);
  } catch {
    return null;
  }
}

/** A request consumes one committed measurement; observers only refresh availability. */
export function useGraphFit(
  surface: RefObject<HTMLElement | SVGSVGElement | null>,
  measure: () => GraphCamera | null,
  apply: (camera: GraphCamera) => void,
  context: string,
) {
  const actions = useRef({ measure, apply });
  const frame = useRef(0);
  const pending = useRef(false);
  const [available, setAvailable] = useState(false);
  const refresh = useCallback(() => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      const camera = actions.current.measure();
      setAvailable(Boolean(camera));
      if (pending.current) {
        pending.current = false;
        if (camera) actions.current.apply(camera);
      }
    });
  }, []);
  const cancel = useCallback(() => {
    pending.current = false;
  }, []);
  const request = useCallback(() => {
    pending.current = true;
    refresh();
  }, [refresh]);
  useLayoutEffect(() => {
    actions.current = { measure, apply };
    refresh();
  });
  useEffect(() => {
    cancel();
    const element = surface.current;
    if (!element) return;
    const scope =
      element.closest(".mesh-graph, .agent-graph-canvas") ?? element;
    const resize = new ResizeObserver(refresh);
    resize.observe(element);
    resize.observe(scope);
    const mutation = new MutationObserver(refresh);
    mutation.observe(scope, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
      attributeFilter: ["class", "style", "hidden"],
    });
    scope.addEventListener("pointerdown", cancel, true);
    scope.addEventListener("wheel", cancel, true);
    window.addEventListener("resize", refresh);
    window.addEventListener("scroll", refresh, true);
    refresh();
    return () => {
      cancel();
      cancelAnimationFrame(frame.current);
      frame.current = 0;
      resize.disconnect();
      mutation.disconnect();
      scope.removeEventListener("pointerdown", cancel, true);
      scope.removeEventListener("wheel", cancel, true);
      window.removeEventListener("resize", refresh);
      window.removeEventListener("scroll", refresh, true);
    };
  }, [surface, context, cancel, refresh]);
  return { available, request, cancel, refresh };
}
