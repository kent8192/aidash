import { useLayoutEffect, useRef, useState } from "react";
import { ArrowRight, Hourglass, Plus } from "lucide-react";
import type { Discovery, HumanRequest, State, Task } from "../../types";
import type { Selection } from "../details";
import type { LocatedRun } from "../workspace-model";
import { useAgentLabel, useEntryLabel, useI18n } from "../../ui";
import { cn } from "../../lib/utils";
import { Avatar } from "../avatar";
import { reference } from "../mesh-model";
import { ChannelRequests } from "../requests";
import { collaborationCopy } from "../copy";
import { Button } from "../../components/ui/button";
import {
  anchorRequests,
  requestCanvas,
  runDisplayStatus,
  type CanvasEdge,
  type Participant,
} from "./canvas-model";
import { Phase } from "./progress";
import { EmptyState } from "../../components/patterns";

type Point = { x: number; y: number };
type Geometry = {
  width: number;
  height: number;
  edges: { edge: CanvasEdge; path: string; label: Point }[];
};

const canvasCopy = {
  "ja-JP": {
    canvas: "依頼のキャンバス",
    home: "自ノード",
    peer: "ピア",
    agents: "エージェント",
    noAgents:
      "まだ参加しているエージェントはいません。タスクを追加すると、担当がここに表示されます。",
    step: "ステップ",
    handoff: "引き継ぎ",
    live: "実行中",
    waiting: "依存・待機",
    waitingFor: (title: string) => `${title} の完了待ち`,
    noTask: "担当タスクなし",
  },
  "en-US": {
    canvas: "Request canvas",
    home: "Home node",
    peer: "Peer",
    agents: "agents",
    noAgents:
      "No agents are working on this request yet. Add a task and its owner appears here.",
    step: "step",
    handoff: "Hand-off",
    live: "Running",
    waiting: "Dependency · waiting",
    waitingFor: (title: string) => `Waiting for ${title}`,
    noTask: "No assigned task",
  },
} as const;

/** Horizontal-tangent bezier between two anchor points. */
function curve(a: Point, b: Point) {
  const dx = Math.max(32, Math.abs(b.x - a.x) * 0.5);
  const sign = b.x >= a.x ? 1 : -1;
  return `M${a.x},${a.y} C${a.x + dx * sign},${a.y} ${b.x - dx * sign},${b.y} ${b.x},${b.y}`;
}

type Box = { left: number; right: number; top: number; bottom: number };

function relativeBox(rect: DOMRect, origin: DOMRect): Box {
  return {
    left: rect.left - origin.left,
    right: rect.right - origin.left,
    top: rect.top - origin.top,
    bottom: rect.bottom - origin.top,
  };
}

/** Edge anchor height: the tile header, so edges stay level across tiles of different heights. */
const anchorY = (box: Box) =>
  box.top + Math.min((box.bottom - box.top) / 2, 44);

/**
 * Side-to-side bezier between tiles; when another tile sits in the way the edge arcs over it,
 * vertical neighbours connect bottom to top.
 */
function edgeGeometry(a: Box, b: Box, obstacles: Box[]) {
  const forward = b.left >= a.right;
  if (forward || b.right <= a.left) {
    const start = { x: forward ? a.right : a.left, y: anchorY(a) };
    const end = { x: forward ? b.left : b.right, y: anchorY(b) };
    const [x0, x1] = [Math.min(start.x, end.x), Math.max(start.x, end.x)];
    const [y0, y1] = [Math.min(start.y, end.y), Math.max(start.y, end.y)];
    const blocking = obstacles.filter(
      (box) =>
        box.right > x0 &&
        box.left < x1 &&
        box.bottom > y0 - 8 &&
        box.top < y1 + 8,
    );
    if (blocking.length === 0)
      return { path: curve(start, end), label: midpoint(start, end) };
    const apex = Math.min(...blocking.map((box) => box.top), a.top, b.top) - 12;
    // A symmetric cubic peaks at (start + end) / 8 + 3/4 of the control height.
    const control = (apex - (start.y + end.y) / 8) / 0.75;
    const sign = forward ? 1 : -1;
    return {
      path: `M${start.x},${start.y} C${start.x + 48 * sign},${control} ${end.x - 48 * sign},${control} ${end.x},${end.y}`,
      label: { x: (start.x + end.x) / 2, y: apex },
    };
  }
  const down = b.top >= a.bottom;
  const start = { x: (a.left + a.right) / 2, y: down ? a.bottom : a.top };
  const end = { x: (b.left + b.right) / 2, y: down ? b.top : b.bottom };
  return {
    path: `M${start.x},${start.y} L${end.x},${end.y}`,
    label: midpoint(start, end),
  };
}

function midpoint(a: Point, b: Point) {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

const edgeStroke: Record<CanvasEdge["tone"], string> = {
  idle: "stroke-edge",
  live: "stroke-brand-line motion-reduce:stroke-brand-mark",
  waiting: "stroke-warning [stroke-dasharray:5_5]",
};

function AgentTile({
  participant,
  name,
  model,
  local,
  register,
  onOpen,
}: {
  participant: Participant;
  name: string;
  model?: string;
  local: boolean;
  register: (element: HTMLElement | null) => void;
  onOpen: () => void;
}) {
  const { locale } = useI18n();
  const words = canvasCopy[locale];
  const status = participant.run
    ? runDisplayStatus(participant.run.run)
    : participant.task?.status;
  const live = status === "THINKING" || status === "TOOL_CALL";
  return (
    <button
      ref={register}
      type="button"
      onClick={onOpen}
      className={cn(
        "relative z-10 grid w-full gap-2 rounded-lg border bg-surface p-3 text-left transition-colors hover:border-border-strong hover:bg-raised",
        live
          ? "border-brand-line shadow-[0_0_0_1px_var(--brand-line)]"
          : status === "WAITING" || status === "BLOCKED" || status === "PAUSED"
            ? "border-warning/45"
            : "border-border",
      )}
    >
      <span className="flex items-start gap-2.5">
        <span className="relative shrink-0">
          <Avatar name={name} />
          {live && (
            <span
              aria-hidden
              className="absolute -inset-1 rounded-lg border border-brand-mark animate-pulse motion-reduce:animate-none"
            />
          )}
        </span>
        <span className="grid min-w-0 flex-1">
          <span className="flex items-center justify-between gap-2">
            <span className="truncate text-[13px] font-semibold text-foreground">
              {name}
            </span>
            <span
              className={cn(
                "shrink-0 rounded-sm border px-1 font-mono text-[10px] leading-4",
                local
                  ? "border-border-strong text-muted-foreground"
                  : "border-brand-line text-brand",
              )}
            >
              {participant.node.replace(/^aidash:\/\//, "")}
            </span>
          </span>
          <span className="truncate font-mono text-[11px] text-faint">
            {model ?? "\u00a0"}
          </span>
        </span>
      </span>
      <span aria-hidden className="-mx-3 h-px bg-border" />
      <span className="flex items-center justify-between gap-2">
        {status ? <Phase status={status} /> : <span />}
        {participant.run && (
          <span className="font-mono text-[11px] tabular text-faint">
            {words.step} {participant.run.run.step}
          </span>
        )}
      </span>
      <span className="line-clamp-2 min-h-[2lh] text-xs leading-normal text-muted-foreground">
        {participant.task?.title ?? words.noTask}
      </span>
    </button>
  );
}

/** Live canvas of one request: agent tiles per node zone, hand-off edges and anchored decisions. */
export function RequestCanvas({
  data,
  discovery,
  tasks,
  runs,
  requests,
  open,
  addTask,
}: {
  data: State;
  discovery?: Discovery;
  tasks: Task[];
  runs: LocatedRun[];
  requests: { request: HumanRequest; node: string }[];
  open: (selection: Selection) => void;
  addTask: () => void;
}) {
  const { locale } = useI18n();
  const words = canvasCopy[locale];
  const agentLabel = useAgentLabel(data, discovery);
  const entryLabel = useEntryLabel(data.registry);
  const { participants, edges, zones } = requestCanvas(
    tasks,
    runs,
    data.node.id,
  );
  const { anchored, loose } = anchorRequests(requests, participants, runs);
  const content = useRef<HTMLDivElement>(null);
  const tiles = useRef(new Map<string, HTMLElement>());
  const [geometry, setGeometry] = useState<Geometry>({
    width: 0,
    height: 0,
    edges: [],
  });
  const edgeSignature = JSON.stringify(
    edges.map((edge) => [edge.key, edge.from, edge.to, edge.tone]),
  );
  useLayoutEffect(() => {
    const element = content.current;
    if (!element) return;
    const measure = () => {
      const origin = element.getBoundingClientRect();
      const boxes = new Map(
        [...tiles.current].map(([key, tile]) => [
          key,
          relativeBox(tile.getBoundingClientRect(), origin),
        ]),
      );
      const next: Geometry = {
        width: element.scrollWidth,
        height: element.scrollHeight,
        edges: edges.flatMap((edge) => {
          const from = boxes.get(edge.from);
          const to = boxes.get(edge.to);
          if (!from || !to) return [];
          const obstacles = [...boxes]
            .filter(([key]) => key !== edge.from && key !== edge.to)
            .map(([, box]) => box);
          return [{ edge, ...edgeGeometry(from, to, obstacles) }];
        }),
      };
      setGeometry((current) =>
        JSON.stringify(current) === JSON.stringify(next) ? current : next,
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    for (const tile of tiles.current.values()) observer.observe(tile);
    return () => observer.disconnect();
    // Geometry depends on the rendered tiles; the signature captures edge identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [edgeSignature, participants.size, requests.length]);
  const modelOf = (participant: Participant) => {
    const entity =
      participant.node === data.node.id
        ? data.registry.find(
            (entry) =>
              entry.id === participant.agent.id &&
              entry.version === participant.agent.version,
          )
        : discovery?.agents.find(
            (agent) =>
              agent.node_id === participant.node &&
              agent.entity.id === participant.agent.id &&
              agent.entity.version === participant.agent.version,
          )?.entity;
    const model = reference(entity?.config.model);
    return model && entryLabel(model);
  };
  return (
    <section
      aria-label={words.canvas}
      className="canvas-grid relative min-h-0 flex-1 overflow-auto"
    >
      <div
        ref={content}
        className="relative flex min-h-full w-max min-w-full flex-wrap content-start items-start gap-x-10 gap-y-6 p-5 pb-14"
      >
        {participants.size === 0 && loose.length === 0 && (
          <EmptyState
            className="max-w-sm bg-background/80"
            title={
              locale === "ja-JP" ? "キャンバスは空です" : "The canvas is empty"
            }
            action={
              <Button
                variant="outline"
                size="sm"
                type="button"
                onClick={addTask}
              >
                <Plus />
                {collaborationCopy[locale].newTask}
              </Button>
            }
          >
            {words.noAgents}
          </EmptyState>
        )}
        {loose.length > 0 && (
          <ChannelRequests
            className="relative z-20 w-72"
            requests={loose}
            localNode={data.node.id}
            open={open}
          />
        )}
        {zones.map((zone) => {
          const count = zone.columns.reduce(
            (total, column) => total + column.length,
            0,
          );
          return (
            <section
              key={zone.node}
              aria-label={zone.node}
              className={cn(
                "rounded-lg border border-dashed px-4 pb-4 pt-3",
                zone.local
                  ? "border-border-strong bg-background/40"
                  : "border-brand-line/60 bg-brand-soft/30",
              )}
            >
              <header className="mb-6 flex items-baseline gap-2 text-[11px]">
                <span className="font-mono text-muted-foreground">
                  {zone.node}
                </span>
                <span className="text-faint">
                  {zone.local ? words.home : words.peer} ·{" "}
                  <span className="font-mono tabular">{count}</span>{" "}
                  {words.agents}
                </span>
              </header>
              <div className="flex items-start gap-28">
                {zone.columns.map((column, index) => (
                  <div key={index} className="grid w-60 min-w-0 gap-8">
                    {column.map((participant) => {
                      const name = agentLabel(
                        participant.node,
                        participant.agent,
                      );
                      const pending = anchored.get(participant.key) ?? [];
                      return (
                        <div key={participant.key} className="grid gap-2">
                          <AgentTile
                            participant={participant}
                            name={name}
                            model={modelOf(participant)}
                            local={zone.local}
                            register={(element) => {
                              if (element)
                                tiles.current.set(participant.key, element);
                              else tiles.current.delete(participant.key);
                            }}
                            onOpen={() => {
                              if (participant.run)
                                open({ kind: "run", ...participant.run });
                              else if (participant.task)
                                open({
                                  kind: "taskDetail",
                                  task: participant.task,
                                });
                            }}
                          />
                          {pending.length > 0 && (
                            <div className="relative z-20 min-w-0 before:absolute before:-top-2 before:left-6 before:h-2 before:border-l before:border-dashed before:border-warning">
                              <ChannelRequests
                                requests={pending}
                                localNode={data.node.id}
                                open={open}
                                requester={name}
                              />
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                ))}
              </div>
            </section>
          );
        })}
        <svg
          aria-hidden
          className="pointer-events-none absolute left-0 top-0 overflow-visible"
          width={geometry.width}
          height={geometry.height}
        >
          {geometry.edges.map(({ edge, path }) => (
            <g key={edge.key}>
              <path
                d={path}
                className={cn("fill-none stroke-[1.5]", edgeStroke[edge.tone])}
              />
              {edge.tone === "live" && (
                <path
                  d={path}
                  strokeDasharray="3 9"
                  className="fill-none stroke-brand-mark stroke-[1.5] motion-reduce:hidden"
                >
                  <animate
                    attributeName="stroke-dashoffset"
                    from="0"
                    to="-12"
                    dur="1.1s"
                    repeatCount="indefinite"
                  />
                </path>
              )}
            </g>
          ))}
        </svg>
        {geometry.edges.map(({ edge, label }) => {
          const text = edge.waitingFor
            ? words.waitingFor(edge.waitingFor.title)
            : edge.task.title;
          return (
            <span
              key={edge.key}
              title={text}
              style={{ left: label.x, top: label.y }}
              className={cn(
                "absolute z-[15] flex max-w-[104px] -translate-x-1/2 -translate-y-1/2 items-center gap-1 rounded-md border bg-background px-1.5 py-0.5 text-[11px] leading-4",
                edge.tone === "waiting"
                  ? "border-warning/40 text-warning"
                  : edge.tone === "live"
                    ? "border-brand-line text-brand"
                    : "border-border text-muted-foreground",
              )}
            >
              {edge.waitingFor ? (
                <Hourglass aria-hidden className="size-3 shrink-0" />
              ) : (
                <ArrowRight aria-hidden className="size-3 shrink-0" />
              )}
              <span className="truncate">{text}</span>
            </span>
          );
        })}
      </div>
      {edges.length > 0 && (
        <div
          aria-hidden
          className="pointer-events-none sticky bottom-0 left-0 flex w-max gap-4 px-5 pb-3 text-[11px] text-faint"
        >
          <span className="inline-flex items-center gap-1.5">
            <i className="inline-block w-4 border-t-[1.5px] border-edge" />
            {words.handoff}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <i className="inline-block w-4 border-t-[1.5px] border-brand-mark" />
            {words.live}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <i className="inline-block w-4 border-t-[1.5px] border-dashed border-warning" />
            {words.waiting}
          </span>
        </div>
      )}
    </section>
  );
}
