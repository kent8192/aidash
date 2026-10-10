// PROTOTYPE ONLY. Observation desk: KPI line, swimlanes, runs, mesh, live event log.
import { useState } from "react";
import { ChevronDown, Maximize2, Pause } from "lucide-react";
import { agents, events, peers, runs, statusLabel, tasks } from "../data";
import type { EventKind } from "../data";
import { Glyph, NOW_MIN, Pane, PaneHeader, nodeLabel, statusTone, toMinutes, toneColor } from "./ui";

const AXIS_START = toMinutes("09:00");
const AXIS_SPAN = 45;
const pct = (min: number) => ((min - AXIS_START) / AXIS_SPAN) * 100;

const ROW_H = 26;
const LANE_PAD = 8;
const BAR_H = 20;
/** Conservative px-per-minute used to keep overflowing bar labels from colliding. */
const LABEL_PX_PER_MIN = 14;

type PlacedTask = (typeof tasks)[number] & { row: number; y: number; start: number; end: number };

const lanes = (() => {
  let top = 0;
  return agents.map((agent) => {
    const rowsEnd: number[] = [];
    const own = tasks
      .filter((t) => t.owner === agent.id)
      .map((t) => {
        const start = toMinutes(t.startedAt);
        const end = t.finishedAt ? toMinutes(t.finishedAt) : NOW_MIN;
        const labelEnd = start + (t.title.length * 12 + 24) / LABEL_PX_PER_MIN;
        let row = rowsEnd.findIndex((e) => e <= start);
        if (row === -1) row = rowsEnd.length;
        rowsEnd[row] = Math.max(end, labelEnd);
        return { ...t, row, start, end, y: 0 };
      });
    const rows = Math.max(rowsEnd.length, 1);
    const height = rows * ROW_H + LANE_PAD * 2;
    const placed: PlacedTask[] = own.map((t) => ({ ...t, y: top + LANE_PAD + t.row * ROW_H + ROW_H / 2 }));
    const lane = { agent, top, height, tasks: placed };
    top += height;
    return lane;
  });
})();
const placedTasks = lanes.flatMap((l) => l.tasks);
const lanesHeight = lanes.reduce((sum, l) => sum + l.height, 0);

const kindList: EventKind[] = ["task", "run", "tool", "request", "artifact", "peer", "error"];

export function ObserveScreen() {
  const counts = {
    running: tasks.filter((t) => t.status === "RUNNING").length,
    blocked: tasks.filter((t) => t.status === "BLOCKED").length,
    done: tasks.filter((t) => t.status === "COMPLETED").length,
    failed: tasks.filter((t) => t.status === "FAILED").length,
  };
  const tokens = runs.reduce((sum, r) => sum + r.tokensIn + r.tokensOut, 0);
  const toolCalls = runs.reduce((sum, r) => sum + r.toolCalls, 0);
  const kpis: { label: string; value: string; tone?: string }[] = [
    { label: "実行中", value: String(counts.running), tone: toneColor.accent },
    { label: "保留", value: String(counts.blocked), tone: toneColor.warn },
    { label: "完了", value: String(counts.done), tone: toneColor.done },
    { label: "失敗", value: String(counts.failed) },
    { label: "トークン", value: `${Math.round(tokens / 1000)}k` },
    { label: "ツール呼び出し", value: String(toolCalls) },
    { label: "経過", value: "41m" },
  ];

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-11 shrink-0 items-center gap-6 border-b border-(--a-line) bg-(--a-surface) px-4">
        <button type="button" className="a-btn a-btn-ghost -ml-2 h-7 gap-1.5 px-2 text-(--a-text)">
          <span className="mono text-[13px] font-semibold">v0.1-release</span>
          <ChevronDown size={13} className="text-(--a-muted)" />
        </button>
        <dl className="m-0 flex items-baseline gap-5">
          {kpis.map((k) => (
            <div key={k.label} className="flex items-baseline gap-1.5">
              <dt className="text-[12px] text-(--a-muted)">{k.label}</dt>
              <dd className="mono m-0 text-[14px] font-medium" style={k.tone ? { color: k.tone } : undefined}>
                {k.value}
              </dd>
            </div>
          ))}
        </dl>
        <span className="flex-1" />
        <div className="a-seg" role="group" aria-label="期間">
          <button type="button" aria-pressed>
            45分
          </button>
          <button type="button">6時間</button>
          <button type="button">24時間</button>
        </div>
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_480px]">
        <div className="a-scroll min-h-0 bg-(--a-surface) pb-20">
          <Swimlanes />
          <RunsTable />
          <MiniMesh />
        </div>
        <EventLog />
      </div>
    </div>
  );
}

function Swimlanes() {
  const ticks = Array.from({ length: 10 }, (_, i) => AXIS_START + i * 5);
  return (
    <section className="border-b border-(--a-line)">
      <div className="flex h-10 items-center gap-3 px-4">
        <h2 className="m-0 text-[13px] font-semibold">タスク</h2>
        <span className="mono text-[12px] text-(--a-faint)">{tasks.length}</span>
        <span className="flex-1" />
        <Legend />
      </div>
      <div className="grid grid-cols-[148px_minmax(0,1fr)] pr-14">
        <div className="h-6 border-b border-(--a-line)" />
        <div className="relative h-6 border-b border-(--a-line)">
          {ticks.filter((t) => Math.abs(t - NOW_MIN) >= 3).map((t) => (
            <span
              key={t}
              className="mono absolute top-1 -translate-x-1/2 text-[11px] text-(--a-faint)"
              style={{ left: `${pct(t)}%` }}
            >
              {`09:${String(t - AXIS_START).padStart(2, "0")}`}
            </span>
          ))}
          <span
            className="mono absolute top-1 -translate-x-1/2 rounded-[4px] bg-(--a-accent) px-1 text-[11px] leading-[16px] text-white"
            style={{ left: `${pct(NOW_MIN)}%` }}
          >
            09:42
          </span>
        </div>

        <div className="relative" style={{ height: lanesHeight }}>
          {lanes.map((l) => (
            <div
              key={l.agent.id}
              className="absolute inset-x-0 flex flex-col justify-center border-b border-(--a-line-soft) pl-4 leading-[1.35]"
              style={{ top: l.top, height: l.height }}
            >
              <span className="flex items-center gap-1.5">
                <span className="font-semibold">{l.agent.name}</span>
                <span className="mono text-[11px] text-(--a-faint)">{nodeLabel(l.agent.node)}</span>
              </span>
              <span className="flex items-center gap-1.5 text-[11.5px]" style={{ color: toneColor[statusTone(l.agent.phase)] }}>
                {statusLabel[l.agent.phase]}
              </span>
            </div>
          ))}
        </div>

        <div className="relative" style={{ height: lanesHeight }}>
          {ticks.map((t) => (
            <span
              key={t}
              aria-hidden
              className="absolute inset-y-0 w-px bg-(--a-line-soft)"
              style={{ left: `${pct(t)}%` }}
            />
          ))}
          {lanes.map((l) => (
            <span
              key={l.agent.id}
              aria-hidden
              className="absolute inset-x-0 border-b border-(--a-line-soft)"
              style={{ top: l.top, height: l.height }}
            />
          ))}
          <svg
            aria-hidden
            className="pointer-events-none absolute inset-0 h-full w-full overflow-visible"
            viewBox={`0 0 100 ${lanesHeight}`}
            preserveAspectRatio="none"
          >
            {placedTasks.flatMap((t) =>
              t.dependencies.map((depId) => {
                const dep = placedTasks.find((d) => d.id === depId)!;
                if (dep.y === t.y) return null;
                const x1 = pct(dep.end);
                const x2 = pct(t.start);
                const d =
                  x2 >= x1 + 1
                    ? `M ${x1} ${dep.y} H ${x1 + 0.8} V ${t.y} H ${x2}`
                    : `M ${Math.max(x2 - 0.8, pct(dep.start) + 0.8)} ${dep.y + BAR_H / 2} V ${t.y} H ${x2}`;
                return (
                  <path
                    key={`${depId}-${t.id}`}
                    d={d}
                    fill="none"
                    stroke={t.status === "BLOCKED" ? toneColor.warn : "#9aa3b2"}
                    strokeWidth={1}
                    strokeDasharray={t.status === "BLOCKED" ? "3 2" : undefined}
                    vectorEffect="non-scaling-stroke"
                  />
                );
              }),
            )}
          </svg>
          <span
            aria-hidden
            className="absolute inset-y-0 w-px bg-(--a-accent)"
            style={{ left: `${pct(NOW_MIN)}%` }}
          />
          {placedTasks.map((t) => {
            const tone = statusTone(t.status);
            const fill =
              t.status === "COMPLETED" ? "#e6efe9" : t.status === "RUNNING" ? "#e3e9fb" : "#fbf3e4";
            return (
              <button
                key={t.id}
                type="button"
                title={`${t.id} · ${t.title} · ${statusLabel[t.status]}`}
                className="group absolute flex cursor-pointer items-center border-0 bg-transparent p-0 text-left"
                style={{
                  left: `${pct(t.start)}%`,
                  width: `${pct(t.end) - pct(t.start)}%`,
                  top: t.y - BAR_H / 2,
                  height: BAR_H,
                }}
              >
                <span
                  className={`absolute inset-0 rounded-[3px] transition-[filter] group-hover:brightness-[0.97] ${t.status === "RUNNING" ? "a-bar-live" : ""} ${t.status === "BLOCKED" ? "a-hatch" : ""}`}
                  style={{
                    background: t.status === "BLOCKED" ? undefined : fill,
                    boxShadow: `inset 2px 0 0 ${toneColor[tone]}${t.status === "BLOCKED" ? `, inset 0 0 0 1px ${toneColor.warn}55` : ""}`,
                  }}
                />
                <span className="relative pl-2 text-[12px] leading-none whitespace-nowrap text-(--a-text)">
                  {t.title}
                </span>
              </button>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function Legend() {
  const items: [string, string, string][] = [
    ["完了", "#e6efe9", toneColor.done],
    ["実行中", "#e3e9fb", toneColor.accent],
    ["保留", "#fbf3e4", toneColor.warn],
  ];
  return (
    <span className="flex items-center gap-3 text-[12px] text-(--a-muted)">
      {items.map(([label, fill, edge]) => (
        <span key={label} className="flex items-center gap-1.5">
          <span className="inline-block h-2.5 w-4 rounded-[2px]" style={{ background: fill, boxShadow: `inset 2px 0 0 ${edge}` }} />
          {label}
        </span>
      ))}
      <span className="flex items-center gap-1.5">
        <span className="inline-block h-px w-4 bg-[#9aa3b2]" />
        依存
      </span>
    </span>
  );
}

function RunsTable() {
  return (
    <section className="border-b border-(--a-line)">
      <div className="flex h-10 items-center gap-3 px-4">
        <h2 className="m-0 text-[13px] font-semibold">実行</h2>
        <span className="mono text-[12px] text-(--a-faint)">{runs.length}</span>
      </div>
      <table className="a-table">
        <colgroup>
          <col style={{ width: 84 }} />
          <col />
          <col style={{ width: 76 }} />
          <col style={{ width: 120 }} />
          <col style={{ width: 56 }} />
          <col style={{ width: 84 }} />
          <col style={{ width: 76 }} />
          <col style={{ width: 64 }} />
          <col style={{ width: 92 }} />
        </colgroup>
        <thead>
          <tr>
            <th className="pl-4">run</th>
            <th>エージェント</th>
            <th>タスク</th>
            <th>フェーズ</th>
            <th className="text-right">step</th>
            <th className="text-right">入力</th>
            <th className="text-right">出力</th>
            <th className="text-right">ツール</th>
            <th className="pr-4 text-right">経過</th>
          </tr>
        </thead>
        <tbody className="mono text-[12px]">
          {runs.map((r) => {
            const tone = statusTone(r.phase);
            return (
              <tr key={r.id}>
                <td className="pl-4">{r.id}</td>
                <td className="sans">{agents.find((a) => a.id === r.agent)?.name}</td>
                <td className="text-(--a-muted)">{r.task}</td>
                <td className="sans" style={{ color: toneColor[tone] }}>
                  <span className="inline-flex items-center gap-1.5">
                    {r.phase === "THINKING" ? <span className="a-pulse" /> : <Glyph tone={tone} size={6} />}
                    {statusLabel[r.phase]}
                  </span>
                </td>
                <td className="text-right">{r.step}</td>
                <td className="text-right">{r.tokensIn.toLocaleString("ja-JP")}</td>
                <td className="text-right">{r.tokensOut.toLocaleString("ja-JP")}</td>
                <td className="text-right">{r.toolCalls}</td>
                <td className="pr-4 text-right">{r.duration}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}

/* Mini mesh: nodes as regions, agents inside, orthogonal edges. Uniformly scaled SVG. */
const meshAgents = [
  { id: "publisher", x: 32, region: "home", tool: null },
  { id: "researcher", x: 196, region: "home", tool: "github" },
  { id: "coder", x: 360, region: "home", tool: "cargo" },
  { id: "verifier", x: 604, region: "lab", tool: "chaos" },
] as const;
const AG_Y = 62;
const AG_W = 148;
const AG_H = 62;

function MiniMesh() {
  const regions = [
    { name: "home", uri: "aidash://home", x: 16, w: 512, status: "local", latency: null as number | null },
    ...peers.map((p, i) => ({
      name: p.name,
      uri: p.id,
      x: i === 0 ? 588 : 808,
      w: i === 0 ? 180 : 136,
      status: p.status,
      latency: p.latencyMs,
    })),
  ];
  const center = (id: string) => meshAgents.find((a) => a.id === id)!;
  const pub = center("publisher");
  const ver = center("verifier");
  return (
    <section>
      <div className="flex h-10 items-center gap-3 px-4">
        <h2 className="m-0 text-[13px] font-semibold">メッシュ</h2>
        <span className="mono text-[12px] text-(--a-faint)">3 ノード · {agents.length} エージェント</span>
        <span className="flex-1" />
        <span className="flex items-center gap-3 text-[12px] text-(--a-muted)">
          <span className="flex items-center gap-1.5">
            <span className="inline-block h-px w-4 bg-(--a-text)" />
            handoff
          </span>
          <span className="flex items-center gap-1.5">
            <span className="inline-block w-4 border-t border-dashed border-[#9aa3b2]" />
            depends
          </span>
        </span>
        <button type="button" className="a-btn a-btn-ghost h-7 px-2">
          <Maximize2 size={13} />
          グラフを開く
        </button>
      </div>
      <div className="px-4 pb-4">
        <svg viewBox="0 0 960 214" className="block h-auto w-full max-w-[1100px]" role="img" aria-label="エージェントメッシュ">
          <defs>
            <marker id="proto-a-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
              <path d="M0 0 L8 4 L0 8 z" fill="#0f1419" />
            </marker>
            <marker id="proto-a-arrow-muted" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
              <path d="M0 0 L8 4 L0 8 z" fill="#9aa3b2" />
            </marker>
          </defs>

          {/* peer links between regions */}
          <line x1={528} y1={30} x2={588} y2={30} stroke="#9aa3b2" strokeWidth={1} />
          <line x1={768} y1={30} x2={808} y2={30} stroke={toneColor.warn} strokeWidth={1} strokeDasharray="3 3" />

          {regions.map((r) => (
            <g key={r.name}>
              <rect x={r.x} y={10} width={r.w} height={150} rx={6} fill="#f7f8fa" stroke="#e4e7ec" />
              <text x={r.x + 12} y={34} fontFamily="Geist Mono" fontSize={12} fontWeight={600} fill="#0f1419">
                {r.name}
              </text>
              <text x={r.x + r.w - 12} y={34} textAnchor="end" fontFamily="Geist Mono" fontSize={11} fill={r.status === "degraded" ? toneColor.warn : "#7c8594"}>
                {r.latency === null ? "このノード" : `${r.latency}ms`}
              </text>
              {r.status === "degraded" ? (
                <>
                  <rect x={r.x + 12} y={AG_Y} width={r.w - 24} height={AG_H} rx={6} fill="none" stroke="#e4e7ec" strokeDasharray="3 3" />
                  <text x={r.x + r.w / 2} y={AG_Y + 28} textAnchor="middle" fontFamily="IBM Plex Sans JP" fontSize={11} fill="#7c8594">
                    エージェントなし
                  </text>
                  <text x={r.x + r.w / 2} y={AG_Y + 46} textAnchor="middle" fontFamily="IBM Plex Sans JP" fontSize={11} fill={toneColor.warn}>
                    応答遅延
                  </text>
                </>
              ) : null}
            </g>
          ))}

          {/* handoff edges */}
          <line x1={196 + AG_W} y1={AG_Y + AG_H / 2} x2={360} y2={AG_Y + AG_H / 2} stroke="#0f1419" strokeWidth={1} markerEnd="url(#proto-a-arrow)" />
          <line x1={360 + AG_W} y1={AG_Y + AG_H / 2} x2={604} y2={AG_Y + AG_H / 2} stroke="#0f1419" strokeWidth={1} markerEnd="url(#proto-a-arrow)" />
          {/* depends edge, routed below the regions */}
          <path
            d={`M ${ver.x + AG_W / 2} ${AG_Y + AG_H} V 186 H ${pub.x + AG_W / 2} V ${AG_Y + AG_H + 1}`}
            fill="none"
            stroke="#9aa3b2"
            strokeDasharray="3 3"
            markerEnd="url(#proto-a-arrow-muted)"
          />
          <rect x={360} y={178} width={150} height={16} fill="#ffffff" />
          <text x={435} y={190} textAnchor="middle" fontFamily="Geist Mono" fontSize={11} fill="#7c8594">
            task-5 は task-4 を待機
          </text>

          {meshAgents.map((m) => {
            const agent = agents.find((a) => a.id === m.id)!;
            const tone = statusTone(agent.phase);
            const live = agent.phase === "THINKING";
            return (
              <g key={m.id}>
                <rect
                  x={m.x}
                  y={AG_Y}
                  width={AG_W}
                  height={AG_H}
                  rx={6}
                  fill="#ffffff"
                  stroke={live ? toneColor.accent : "#d9dde4"}
                  strokeWidth={live ? 1.5 : 1}
                />
                <rect x={m.x + 12} y={AG_Y + 14} width={7} height={7} fill={toneColor[tone]} />
                <text x={m.x + 26} y={AG_Y + 22} fontFamily="Geist" fontSize={12.5} fontWeight={600} fill="#0f1419">
                  {agent.name}
                </text>
                <text x={m.x + AG_W - 10} y={AG_Y + 22} textAnchor="end" fontFamily="IBM Plex Sans JP" fontSize={10.5} fill={toneColor[tone]}>
                  {statusLabel[agent.phase]}
                </text>
                <text x={m.x + 12} y={AG_Y + 40} fontFamily="Geist Mono" fontSize={10.5} fill="#5b6472">
                  {agent.model}
                </text>
                <text x={m.x + 12} y={AG_Y + 54} fontFamily="Geist Mono" fontSize={10.5} fill="#7c8594">
                  {m.tool ? `tool: ${m.tool}` : "tool: なし"}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
    </section>
  );
}

function EventLog() {
  const [kinds, setKinds] = useState<EventKind[]>([]);
  const toggle = (k: EventKind) => setKinds((ks) => (ks.includes(k) ? ks.filter((x) => x !== k) : [...ks, k]));
  const rows = [...events].reverse().filter((e) => kinds.length === 0 || kinds.includes(e.kind));
  return (
    <Pane className="border-l border-(--a-line) bg-(--a-surface)">
      <PaneHeader>
        <h2 className="m-0 text-[13px] font-semibold">イベント</h2>
        <span className="flex items-center gap-1.5 text-[12px] text-(--a-done)">
          <span className="a-pulse" />
          ライブ
        </span>
        <span className="flex-1" />
        <span className="mono text-[12px] text-(--a-faint)">
          {rows.length}/{events.length}
        </span>
        <button type="button" className="a-icon-btn" aria-label="一時停止">
          <Pause size={14} />
        </button>
      </PaneHeader>
      <div className="flex shrink-0 flex-wrap gap-1.5 border-b border-(--a-line) px-4 py-2.5">
        <button type="button" className="a-chip" aria-pressed={kinds.length === 0} onClick={() => setKinds([])}>
          すべて
        </button>
        {kindList.map((k) => (
          <button key={k} type="button" className="a-chip mono" aria-pressed={kinds.includes(k)} onClick={() => toggle(k)}>
            {k}
            <span className="text-(--a-faint)">{events.filter((e) => e.kind === k).length}</span>
          </button>
        ))}
      </div>
      <div className="a-scroll min-h-0 flex-1 pb-16">
        <table className="a-table">
          <colgroup>
            <col style={{ width: 88 }} />
            <col style={{ width: 94 }} />
            <col style={{ width: 80 }} />
            <col />
          </colgroup>
          <thead>
            <tr>
              <th className="pl-4">時刻</th>
              <th>主体</th>
              <th>種別</th>
              <th className="pr-4">内容</th>
            </tr>
          </thead>
          <tbody className="text-[12px]">
            {rows.map((e) => {
              const warn = e.severity === "warn";
              return (
                <tr key={e.at} className={warn ? "a-warn" : undefined}>
                  <td className="mono pl-4 align-top leading-[32px] text-(--a-muted)">{e.at}</td>
                  <td className="mono align-top leading-[32px]">{e.actor}</td>
                  <td className="mono align-top leading-[32px]" style={{ color: warn ? toneColor.warn : "var(--a-faint)" }}>
                    {e.kind}
                  </td>
                  <td className="py-[7px] pr-4 align-top leading-[1.55] whitespace-normal" style={warn ? { color: "#7a5212" } : undefined}>
                    {e.text}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Pane>
  );
}
