// PROTOTYPE ONLY. Variant B shared primitives: geometry hooks, avatars, phase marks, event timeline.
import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import {
  agentName,
  agents,
  events,
  statusLabel,
  tasks,
  type EventKind,
  type RunPhase,
  type TaskStatus,
} from "../data";

export type Decision = "pending" | "approved" | "rejected";

// Scenario clock. Everything is anchored to 09:00 and the workspace's latest update (09:41).
const START = 9 * 60;
const SPAN = 45;
export const NOW = "09:41:00";
export const minutes = (time: string) => {
  const [h, m, s = "0"] = time.split(":");
  return Number(h) * 60 + Number(m) + Number(s) / 60 - START;
};
export const at = (time: string) => `${(minutes(time) / SPAN) * 100}%`;

export function useBox<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) =>
      setBox({ w: entry.contentRect.width, h: entry.contentRect.height }),
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return [ref, box] as const;
}

export const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
export const fmt = (n: number) => n.toLocaleString("ja-JP");

export const phaseTone: Record<string, string> = {
  THINKING: "live",
  TOOL_CALL: "live",
  RUNNING: "live",
  WAITING: "warn",
  BLOCKED: "warn",
  COMPLETED: "done",
  FAILED: "error",
  READY: "idle",
  OPEN: "idle",
  CLAIMED: "idle",
  CANCELLED: "idle",
};

export function Avatar({
  id,
  size = 30,
  phase,
}: {
  id: string;
  size?: number;
  phase?: RunPhase | string;
}) {
  const name = agentName(id);
  const initial = id === "human" ? "田" : id === "system" ? "A" : name.slice(0, 1);
  const style = { "--s": `${size}px` } as CSSProperties;
  return (
    <span className="b-ava" data-phase={phase} data-who={id === "human" ? "human" : id === "system" ? "system" : "agent"} style={style} aria-hidden>
      {initial}
      {phase === "COMPLETED" && <span className="b-ava-done" />}
    </span>
  );
}

export function Phase({ status, children }: { status: string; children?: React.ReactNode }) {
  return (
    <span className="b-phase" data-tone={phaseTone[status] ?? "idle"}>
      <span className="b-dot" />
      {children ?? statusLabel[status] ?? status}
    </span>
  );
}

export const kindMeta: Record<EventKind, { label: string }> = {
  task: { label: "タスク" },
  run: { label: "実行" },
  tool: { label: "ツール" },
  request: { label: "判断" },
  artifact: { label: "成果物" },
  peer: { label: "ピア" },
  error: { label: "エラー" },
};

const axis = Array.from({ length: 10 }, (_, i) => i * 5);
const lanes = [
  { id: "origin", label: "依頼・システム", actors: ["human", "system"] },
  ...agents.map((a) => ({ id: a.id, label: a.name, actors: [a.id] })),
];
// Same-owner tasks that overlap in time go to a second sub-row so both stay readable.
const spanRow: Record<string, number> = {};
for (const t of tasks) {
  const clash = tasks.some(
    (o) =>
      o.owner === t.owner &&
      spanRow[o.id] === 0 &&
      minutes(o.startedAt) < minutes(t.finishedAt ?? NOW) &&
      minutes(t.startedAt) < minutes(o.finishedAt ?? NOW),
  );
  spanRow[t.id] = clash ? 1 : 0;
}

function Tick({
  index,
  selected,
  onSelect,
}: {
  index: number;
  selected: boolean;
  onSelect: (i: number) => void;
}) {
  const e = events[index];
  return (
    <button
      type="button"
      className="b-tick"
      data-kind={e.kind}
      data-sev={e.severity}
      aria-pressed={selected}
      style={{ left: at(e.at) }}
      title={`${e.at}  ${agentName(e.actor)}  ${e.text}`}
      onClick={() => onSelect(index)}
    >
      <span className="b-tick-stem" />
      <span className="b-tick-head" />
    </button>
  );
}

function Axis({ labels }: { labels: boolean }) {
  return (
    <>
      {axis.map((m) => (
        <span key={m} className="b-grid" style={{ left: `${(m / SPAN) * 100}%` }}>
          {labels && <span className="b-grid-label mono">{`09:${String(m).padStart(2, "0")}`}</span>}
        </span>
      ))}
      <span className="b-future" style={{ left: at(NOW) }} />
    </>
  );
}

export function Timeline({ expanded = false }: { expanded?: boolean }) {
  const [selected, setSelected] = useState(events.length - 1);
  const current = events[selected];
  return (
    <section className="b-tl" data-expanded={expanded} aria-label="イベントタイムライン">
      <header className="b-tl-head">
        <span className="b-tl-title">タイムライン</span>
        <span className="mono b-faint">09:00〜09:45</span>
        <span className="b-tl-legend">
          {(Object.keys(kindMeta) as EventKind[]).map((k) => (
            <span key={k} className="b-legend-item">
              <span className="b-tick-head b-legend-glyph" data-kind={k} />
              {kindMeta[k].label}
            </span>
          ))}
        </span>
        <span className="b-tl-current">
          <span className="mono" data-kind={current.kind}>
            {current.at}
          </span>
          <span className="b-muted">{agentName(current.actor)}</span>
          <span className="b-tl-text">{current.text}</span>
        </span>
      </header>
      {expanded ? (
        <div className="b-tl-lanes">
          {lanes.map((lane) => (
            <div key={lane.id} className="b-lane">
              <span className="b-lane-label">
                {lane.id === "origin" ? (
                  <span className="b-lane-origin" />
                ) : (
                  <Avatar id={lane.id} size={16} />
                )}
                {lane.label}
              </span>
              <div className="b-lane-track">
                <Axis labels={false} />
                {tasks
                  .filter((t) => lane.actors.includes(t.owner))
                  .map((t) => (
                    <span
                      key={t.id}
                      className="b-span"
                      data-status={t.status as TaskStatus}
                      data-row={spanRow[t.id]}
                      title={`${t.id} ${t.title} (${statusLabel[t.status]})`}
                      style={{
                        left: at(t.startedAt),
                        width: `calc(${at(t.finishedAt ?? NOW)} - ${at(t.startedAt)})`,
                      }}
                    >
                      <span className="mono">{t.id}</span>
                    </span>
                  ))}
                {events.map((e, i) =>
                  lane.actors.includes(e.actor) ? (
                    <Tick key={i} index={i} selected={i === selected} onSelect={setSelected} />
                  ) : null,
                )}
              </div>
            </div>
          ))}
          <div className="b-lane b-lane-axis">
            <span />
            <div className="b-lane-track">
              <Axis labels />
            </div>
          </div>
          <span className="b-now-wrap">
            <span className="b-now" style={{ left: at(NOW) }}>
              <span className="b-now-label mono">09:41</span>
            </span>
          </span>
        </div>
      ) : (
        <div className="b-tl-strip">
          <span className="b-lane-label">
            <span className="mono">{events.length}</span>
            <span className="b-muted">件のイベント</span>
          </span>
          <div className="b-lane-track">
            <Axis labels />
            {events.map((_, i) => (
              <Tick key={i} index={i} selected={i === selected} onSelect={setSelected} />
            ))}
            <span className="b-now" style={{ left: at(NOW) }}>
              <span className="b-now-label mono">09:41</span>
            </span>
          </div>
        </div>
      )}
    </section>
  );
}
