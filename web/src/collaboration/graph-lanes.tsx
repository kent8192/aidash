import type { Run, Task } from "../types";
import { cn } from "../lib/utils";
import { statusTone, type StatusTone } from "../ui";
import type { MeshCopy } from "./mesh-copy";
import type { MeshNode } from "./mesh-model";
import type { TimelineEvent } from "./graph-timeline";

export type LaneEvent = TimelineEvent & { run?: string; task?: string };

const system = "system";
const minute = 60_000;
const terminal = ["COMPLETED", "FAILED", "CANCELLED"];
const barTone: Record<StatusTone, string> = {
  brand: "border-brand-line bg-brand-soft text-foreground",
  success: "border-success/40 bg-success-soft text-muted-foreground",
  warning: "border-warning/50 bg-warning-soft text-foreground",
  danger: "border-destructive/50 bg-destructive-soft text-foreground",
  neutral: "border-border-strong bg-neutral-soft text-muted-foreground",
};
const steps = [1, 2, 5, 10, 15, 30, 60, 120, 240, 360, 720, 1440, 2880, 10080];

function tickTone(kind: string) {
  if (/fail|block|error|denied/i.test(kind)) return "bg-destructive";
  if (/waiting|human|approval|question/i.test(kind)) return "bg-warning";
  if (/artifact|completed/i.test(kind)) return "bg-success";
  if (/^run\./.test(kind)) return "bg-brand-mark";
  return "bg-edge-strong";
}

/** Per-agent lanes: task bars by status, event ticks and a now marker over the activity window. */
export function ExecutionLanes({
  tasks,
  runs,
  events,
  nodes,
  label,
  select,
  copy,
  locale,
  hours,
  now,
  limit = 80,
}: {
  tasks: Task[];
  runs: { run: Run; node: string }[];
  events: LaneEvent[];
  nodes: MeshNode[];
  label: (node: MeshNode) => string;
  select: (id: string) => void;
  copy: MeshCopy;
  locale: string;
  hours: number;
  now: number;
  limit?: number;
}) {
  const principal = (node: string, agent: string, version: string) =>
    `${node}/agents/${agent}@${version}`;
  const runLane = new Map(
    runs.map(({ run, node }) => [
      run.id,
      principal(node, run.agent_id, run.agent_version),
    ]),
  );
  const taskLane = new Map(
    tasks.map((task) => [
      task.id,
      task.owner ??
        runLane.get(runs.find(({ run }) => run.task_id === task.id)?.run.id ?? "") ??
        system,
    ]),
  );
  const agentNodes = new Map(
    nodes
      .filter((node) => node.kind === "agent" && node.entity)
      .map((node) => [
        principal(node.nodeId, node.entity!.id, node.entity!.version),
        node,
      ]),
  );
  const shown = events.slice(-limit);
  const ticks = shown.flatMap((event) => {
    const at = Date.parse(event.created_at);
    if (!Number.isFinite(at)) return [];
    const lane =
      (event.run && runLane.get(event.run)) ||
      (event.task && taskLane.get(event.task)) ||
      system;
    return [{ event, at, lane }];
  });
  const taskEvents = new Map<string, number[]>();
  for (const tick of ticks)
    if (tick.event.task)
      taskEvents.set(tick.event.task, [
        ...(taskEvents.get(tick.event.task) ?? []),
        tick.at,
      ]);
  const windowStart = hours > 0 ? now - hours * 3_600_000 : -Infinity;
  const bars = tasks.flatMap((task) => {
    const created = Date.parse(task.created_at);
    if (!Number.isFinite(created)) return [];
    const seen = taskEvents.get(task.id) ?? [];
    const end = terminal.includes(task.status)
      ? Math.max(created + minute, ...seen)
      : now;
    if (end < windowStart) return [];
    const node = nodes.find(
      (candidate) =>
        candidate.kind === "task" && candidate.resourceId === task.id,
    );
    return [
      {
        task,
        node,
        start: Math.max(created, windowStart),
        end,
        lane: taskLane.get(task.id) ?? system,
      },
    ];
  });
  const times = [...ticks.map((tick) => tick.at), ...bars.map((bar) => bar.start)];
  if (!times.length)
    return (
      <section
        className="mesh-timeline flex h-12 shrink-0 items-center gap-3 border-t border-border bg-surface px-4"
        aria-label={copy.timeline}
      >
        <h2 className="text-[13px] font-medium">{copy.timeline}</h2>
        <p className="text-xs text-faint">{copy.noTimeline}</p>
      </section>
    );
  const first = Math.max(
    windowStart,
    Math.min(now - 15 * minute, ...times),
  );
  const span = Math.max(now - first, minute);
  const start = first - span * 0.02;
  const end = now + span * 0.04;
  const x = (value: number) => ((value - start) / (end - start)) * 100;
  const stepMinutes =
    steps.find((value) => (end - start) / (value * minute) <= 6) ?? 10080;
  const axis: number[] = [];
  for (
    let value = Math.ceil(start / (stepMinutes * minute)) * stepMinutes * minute;
    value <= end;
    value += stepMinutes * minute
  )
    axis.push(value);
  const longRange = end - start > 24 * 60 * minute;
  const time = (value: number) =>
    new Date(value).toLocaleString(locale, {
      ...(longRange ? { month: "numeric", day: "numeric" } : {}),
      hour: "2-digit",
      minute: "2-digit",
    });
  const keys = [
    ...new Set([
      ...runs.map(({ run, node }) =>
        principal(node, run.agent_id, run.agent_version),
      ),
      ...bars.map((bar) => bar.lane),
      ...ticks.map((tick) => tick.lane),
    ]),
  ].filter(
    (key) =>
      key !== system &&
      (bars.some((bar) => bar.lane === key) ||
        ticks.some((tick) => tick.lane === key)),
  );
  const lanes = [
    ...(bars.some((bar) => bar.lane === system) ||
    ticks.some((tick) => tick.lane === system)
      ? [system]
      : []),
    ...keys,
  ];
  const laneName = (key: string) => {
    if (key === system) return copy.systemLane;
    const node = agentNodes.get(key);
    return node
      ? label(node)
      : (key.split("/agents/")[1]?.split("@")[0] ?? key);
  };
  return (
    <section
      className="mesh-timeline flex h-[212px] shrink-0 flex-col border-t border-border bg-surface max-md:h-[180px]"
      aria-label={copy.timeline}
    >
      <header className="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4">
        <h2 className="text-[13px] font-medium">{copy.timeline}</h2>
        <span className="font-mono text-[11px] text-faint tabular">
          {time(first)} – {time(now)}
        </span>
        <span className="ml-auto flex items-center gap-3 text-[11px] text-faint max-md:hidden">
          <span className="flex items-center gap-1.5">
            <i className="h-2 w-4 rounded-sm border border-border-strong bg-neutral-soft" />
            {copy.legendTask}
          </span>
          <span className="flex items-center gap-1.5">
            <i className="h-2.5 w-0.5 rounded-sm bg-edge-strong" />
            {copy.legendEvent}
          </span>
          <span className="flex items-center gap-1.5">
            <i className="h-2.5 w-0.5 rounded-sm bg-destructive" />
            {copy.legendAlert}
          </span>
        </span>
        <span className="font-mono text-[11px] text-muted-foreground tabular max-md:ml-auto">
          {events.length} {copy.events}
          {events.length > shown.length &&
            ` · ${copy.omitted}: ${events.length - shown.length}`}
        </span>
      </header>
      <div className="min-h-0 flex-1 overflow-auto">
        <div className="relative min-w-[560px]">
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-y-0 left-[148px] right-0"
          >
            {axis.map((value) => (
              <span
                key={value}
                className="absolute inset-y-0 w-px bg-border"
                style={{ left: `${x(value)}%` }}
              />
            ))}
          </div>
          <ol className="mesh-timeline-track relative">
            {lanes.map((key) => {
              const agent = agentNodes.get(key);
              return (
                <li key={key} className="flex h-7 items-center">
                  <span className="sticky left-0 z-[2] flex h-full w-[148px] shrink-0 items-center gap-2 bg-surface px-4 text-xs text-muted-foreground">
                    {agent ? (
                      <button
                        type="button"
                        className="truncate text-left transition-colors hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                        onClick={() => select(agent.id)}
                      >
                        {laneName(key)}
                      </button>
                    ) : (
                      <span className="truncate">{laneName(key)}</span>
                    )}
                  </span>
                  <span className="relative h-full min-w-0 flex-1">
                    {bars
                      .filter((bar) => bar.lane === key)
                      .map((bar) => (
                        <button
                          type="button"
                          key={bar.task.id}
                          disabled={!bar.node}
                          className={cn(
                            "absolute top-1/2 flex h-4 -translate-y-1/2 items-center overflow-hidden rounded-sm border px-1.5 text-left font-mono text-[10.5px] leading-none transition-colors hover:brightness-110 focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default",
                            barTone[statusTone(bar.task.status)],
                          )}
                          style={{
                            left: `${x(bar.start)}%`,
                            width: `max(6px, ${x(bar.end) - x(bar.start)}%)`,
                          }}
                          title={`${bar.task.title} · ${bar.task.status}`}
                          onClick={() => bar.node && select(bar.node.id)}
                        >
                          <span className="truncate">{bar.task.title}</span>
                        </button>
                      ))}
                    {ticks
                      .filter((tick) => tick.lane === key)
                      .map(({ event, at }) => {
                        const target = nodes.find(
                          (node) => node.id === event.reference,
                        );
                        const name = `${event.kind} · ${new Date(at).toLocaleString(locale)}`;
                        return (
                          <button
                            type="button"
                            key={event.id}
                            disabled={!target}
                            className="group absolute top-1/2 z-[1] grid h-5 w-2.5 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-sm focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default"
                            style={{ left: `${x(at)}%` }}
                            title={name}
                            aria-label={name}
                            onClick={() => target && select(target.id)}
                          >
                            <span
                              className={cn(
                                "h-3 w-0.5 rounded-sm ring-2 ring-surface transition-transform group-hover:scale-y-125",
                                tickTone(event.kind),
                              )}
                            />
                          </button>
                        );
                      })}
                  </span>
                </li>
              );
            })}
          </ol>
          <div className="relative ml-[148px] h-6 border-t border-border">
            {axis.map((value) => (
              <time
                key={value}
                dateTime={new Date(value).toISOString()}
                className="absolute top-1 -translate-x-1/2 font-mono text-[10.5px] text-faint tabular"
                style={{ left: `${x(value)}%` }}
              >
                {time(value)}
              </time>
            ))}
          </div>
          <div
            className="pointer-events-none absolute inset-y-0 left-[148px] right-0"
            aria-hidden="true"
          >
            <span
              className="absolute inset-y-0 w-px bg-brand-mark"
              style={{ left: `${x(now)}%` }}
            />
            <span
              className="absolute top-0.5 -translate-x-1/2 rounded-sm bg-primary px-1 font-mono text-[10px] font-medium leading-4 text-primary-foreground tabular"
              style={{ left: `${x(now)}%` }}
              title={copy.now}
            >
              {time(now)}
            </span>
          </div>
        </div>
      </div>
    </section>
  );
}
