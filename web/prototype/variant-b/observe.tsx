// PROTOTYPE ONLY. Variant B observe screen: full federation mesh, filter overlay, inspector, lane timeline.
import { useState } from "react";
import { Cpu, FileText, Maximize, Pause, Search, Server, Wrench, X, ZoomIn, ZoomOut } from "lucide-react";
import {
  agents,
  events,
  mesh,
  node as selfNode,
  peers,
  policies,
  runs,
  statusLabel,
  tasks,
  type MeshKind,
} from "../data";
import { Avatar, Phase, Timeline, clamp, fmt, useBox } from "./shared";

type MeshNode = (typeof mesh.nodes)[number];
type Relation = (typeof mesh.edges)[number]["relation"];
type Pt = { x: number; y: number };
type Rect = { x: number; y: number; w: number; h: number };

const AGENT_W = 152;
const AGENT_H = 56;
const PILL_H = 28;

// Deterministic layout in unit space. Agents sit inside their federation node; models above, tools below.
const unit: Record<string, Pt> = {
  "m-sonnet": { x: 0.0, y: 0.0 },
  "m-codex": { x: 0.42, y: 0.0 },
  "a-researcher": { x: 0.0, y: 0.3 },
  "a-coder": { x: 0.42, y: 0.3 },
  "a-publisher": { x: 0.21, y: 0.64 },
  "t-github": { x: 0.0, y: 1.0 },
  "t-cargo": { x: 0.42, y: 1.0 },
  "a-verifier": { x: 1.0, y: 0.3 },
  "t-chaos": { x: 0.74, y: 0.68 },
};
const edgeRegionUnit = { x: 1.0, y: 0.98 };

const kindLabel: Record<string, string> = { node: "ノード", agent: "エージェント", tool: "ツール", model: "モデル" };
const relationLabel: Record<Relation, string> = {
  handoff: "引き継ぎ",
  depends: "依存",
  uses: "利用",
  model: "モデル",
  peer: "ピア",
};
const kinds = ["node", "agent", "tool", "model"] as MeshKind[];
const relations = Object.keys(relationLabel) as Relation[];

const agentOf = (id: string) => agents.find((a) => `a-${a.id}` === id);
const label = (id: string) => mesh.nodes.find((n) => n.id === id)?.label ?? id;

function Filters({
  hiddenKinds,
  hiddenRelations,
  toggleKind,
  toggleRelation,
}: {
  hiddenKinds: Set<string>;
  hiddenRelations: Set<string>;
  toggleKind: (k: string) => void;
  toggleRelation: (r: string) => void;
}) {
  return (
    <div className="b-overlay b-filters">
      <label className="b-search">
        <Search size={14} className="b-faint" />
        <input placeholder="ノードを検索" aria-label="ノードを検索" />
        <span className="b-kbd">/</span>
      </label>
      <div className="b-ov-group">
        <span className="b-ov-label">種類</span>
        {kinds.map((k) => (
          <button key={k} type="button" className="b-toggle" aria-pressed={!hiddenKinds.has(k)} onClick={() => toggleKind(k)}>
            <span className="b-kind-glyph" data-kind={k} />
            <span className="flex-1">{kindLabel[k]}</span>
            <span className="mono b-faint">{mesh.nodes.filter((n) => n.kind === k).length}</span>
            <span className="b-check" />
          </button>
        ))}
      </div>
      <div className="b-ov-group">
        <span className="b-ov-label">関係</span>
        {relations.map((r) => (
          <button key={r} type="button" className="b-toggle" aria-pressed={!hiddenRelations.has(r)} onClick={() => toggleRelation(r)}>
            <i className="b-lg-line" data-rel={r} />
            <span className="flex-1">{relationLabel[r]}</span>
            <span className="mono b-faint">{mesh.edges.filter((e) => e.relation === r).length}</span>
            <span className="b-check" />
          </button>
        ))}
      </div>
      <div className="b-ov-group b-ov-status">
        <span className="b-ov-label">状態</span>
        <div className="grid grid-cols-3">
          {[
            ["実行中", agents.filter((a) => a.phase === "THINKING").length, "live"],
            ["待機", agents.filter((a) => a.phase === "WAITING").length, "warn"],
            ["完了", agents.filter((a) => a.phase === "COMPLETED").length, "done"],
          ].map(([l, n, tone]) => (
            <span key={l} className="b-ministat" data-tone={tone}>
              <span className="mono">{n}</span>
              <span className="b-faint">{l}</span>
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

function Inspector({ id, onClose }: { id: string; onClose: () => void }) {
  const n = mesh.nodes.find((m) => m.id === id) as MeshNode;
  const agent = agentOf(id);
  const run = agent && runs.find((r) => r.agent === agent.id);
  const task = run && tasks.find((t) => t.id === run.task);
  const links = mesh.edges.filter((e) => e.from === id || e.to === id);
  const budget = policies.find((p) => p.id === "pol-budget")!;
  const relatedEvents = agent
    ? events.filter((e) => e.actor === agent.id)
    : events.filter((e) => e.text.includes(n.label));
  const peer = peers.find((p) => p.name === n.label);
  const nodeAgents = agents.filter((a) => a.node === `aidash://${n.label}`);

  return (
    <aside className="b-inspector" aria-label="インスペクター">
      <header className="b-insp-head">
        <span className="b-faint">
          {kindLabel[n.kind]} · <span className="mono">{n.id}</span>
        </span>
        <button type="button" className="b-ib ml-auto" aria-label="選択を解除" onClick={onClose}>
          <X size={15} />
        </button>
      </header>
      <div className="b-insp-scroll">
        <section className="b-insp-id">
          {agent ? (
            <Avatar id={agent.id} size={40} phase={agent.phase} />
          ) : (
            <span className="b-insp-icon">
              {n.kind === "tool" ? <Wrench size={18} /> : n.kind === "model" ? <Cpu size={18} /> : <Server size={18} />}
            </span>
          )}
          <div className="min-w-0">
            <h2 className={n.kind === "agent" ? "b-insp-name" : "b-insp-name mono"}>{n.label}</h2>
            <p className="b-muted">
              {agent
                ? agent.role
                : n.kind === "node"
                  ? n.label === selfNode.name
                    ? `自ノード · ${selfNode.endpoint}`
                    : `ピアノード · ${peer?.status === "connected" ? "接続中" : "劣化"}`
                  : n.kind === "tool"
                    ? "エージェントが呼び出すツール"
                    : "推論モデル"}
            </p>
          </div>
        </section>

        {agent && run && task && (
          <>
            <section className="b-insp-sec">
              <dl className="b-kv">
                <dt>ノード</dt>
                <dd className="mono">{agent.node}</dd>
                <dt>モデル</dt>
                <dd className="mono">{agent.model}</dd>
                <dt>状態</dt>
                <dd>
                  <Phase status={agent.phase} />
                </dd>
                <dt>タスク</dt>
                <dd>
                  <span className="mono b-faint">{task.id}</span> {task.title}
                  <span className="block b-faint">
                    {statusLabel[task.status]}
                    {task.dependencies.length > 0 && <> · 依存 <span className="mono">{task.dependencies.join(", ")}</span></>}
                  </span>
                </dd>
              </dl>
            </section>
            <section className="b-insp-sec">
              <h3 className="b-sec-title">
                実行 <span className="mono b-faint">{run.id}</span>
              </h3>
              <div className="b-stats">
                {[
                  ["ステップ", String(run.step)],
                  ["経過時間", run.duration],
                  ["入力トークン", fmt(run.tokensIn)],
                  ["出力トークン", fmt(run.tokensOut)],
                  ["ツール呼び出し", String(run.toolCalls)],
                  ["フェーズ", statusLabel[run.phase]],
                ].map(([k, v]) => (
                  <div key={k} className="b-stat">
                    <span className="b-faint">{k}</span>
                    <span className={k === "フェーズ" ? "b-stat-v" : "b-stat-v mono"}>{v}</span>
                  </div>
                ))}
              </div>
              <div className="b-budget">
                <div className="flex items-center justify-between">
                  <span className="b-faint">トークン予算</span>
                  <span className="mono">
                    {fmt(run.tokensIn + run.tokensOut)} <span className="b-faint">/ 200,000</span>
                  </span>
                </div>
                <span className="b-meter">
                  <span style={{ width: `${((run.tokensIn + run.tokensOut) / 200000) * 100}%` }} />
                </span>
                <span className="b-faint">
                  {budget.name} <span className="mono">{budget.id}</span>
                </span>
              </div>
            </section>
          </>
        )}

        {n.kind === "node" && (
          <section className="b-insp-sec">
            <dl className="b-kv">
              <dt>アドレス</dt>
              <dd className="mono">aidash://{n.label}</dd>
              <dt>状態</dt>
              <dd>
                {peer ? (
                  <Phase status={peer.status === "connected" ? "COMPLETED" : "WAITING"}>
                    {peer.status === "connected" ? "接続中" : "劣化"} · <span className="mono">{peer.latencyMs}ms</span>
                  </Phase>
                ) : (
                  <Phase status="COMPLETED">自ノード</Phase>
                )}
              </dd>
              <dt>エージェント</dt>
              <dd>{nodeAgents.length > 0 ? nodeAgents.map((a) => a.name).join(", ") : "なし"}</dd>
            </dl>
          </section>
        )}

        {(n.kind === "tool" || n.kind === "model") && (
          <section className="b-insp-sec">
            <dl className="b-kv">
              <dt>利用元</dt>
              <dd>{links.map((l) => label(l.from === id ? l.to : l.from)).join(", ")}</dd>
              {n.kind === "tool" && (
                <>
                  <dt>ポリシー</dt>
                  <dd>
                    {policies
                      .filter((p) => p.scope === `tool:${n.label}`)
                      .map((p) => (
                        <span key={p.id} className="block">
                          {p.name} <span className="mono b-faint">{p.id}</span>
                        </span>
                      ))}
                    {policies.every((p) => p.scope !== `tool:${n.label}`) && <span className="b-faint">既定</span>}
                  </dd>
                </>
              )}
            </dl>
          </section>
        )}

        <section className="b-insp-sec">
          <h3 className="b-sec-title">
            関係 <span className="mono b-faint">{links.length}</span>
          </h3>
          <ul className="b-links">
            {links.map((l, i) => {
              const other = l.from === id ? l.to : l.from;
              return (
                <li key={i}>
                  <span className="mono b-faint">{l.from === id ? "→" : "←"}</span>
                  <span className={other.startsWith("a-") ? "" : "mono"}>{label(other)}</span>
                  <span className="b-faint ml-auto">{relationLabel[l.relation]}</span>
                </li>
              );
            })}
          </ul>
        </section>

        <section className="b-insp-sec">
          <h3 className="b-sec-title">
            最近のイベント <span className="mono b-faint">{relatedEvents.length}</span>
          </h3>
          {relatedEvents.length === 0 && <p className="b-faint">該当するイベントはありません</p>}
          <ol className="b-evlist">
            {relatedEvents
              .slice()
              .reverse()
              .map((e, i) => (
                <li key={i} data-kind={e.kind} data-sev={e.severity}>
                  <span className="mono b-faint">{e.at}</span>
                  <span>{e.text}</span>
                </li>
              ))}
          </ol>
        </section>
      </div>
      {agent && (
        <footer className="b-insp-foot">
          <button type="button" className="b-btn">
            <FileText size={14} />
            実行ログ
          </button>
          <button type="button" className="b-btn">
            <Pause size={14} />
            一時停止
          </button>
        </footer>
      )}
    </aside>
  );
}

// Choose the anchor on each box facing the other so edges never cross their own node.
function anchors(a: Rect, b: Rect) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const horizontal = Math.abs(dx) / (a.w / 2 + b.w / 2) > Math.abs(dy) / (a.h / 2 + b.h / 2);
  if (horizontal) {
    const s = Math.sign(dx);
    return { a: { x: a.x + (s * a.w) / 2, y: a.y }, b: { x: b.x - (s * b.w) / 2, y: b.y }, horizontal };
  }
  const s = Math.sign(dy);
  return { a: { x: a.x, y: a.y + (s * a.h) / 2 }, b: { x: b.x, y: b.y - (s * b.h) / 2 }, horizontal };
}

function path(a: Pt, b: Pt, horizontal: boolean) {
  if (horizontal) {
    const d = Math.max(30, Math.abs(b.x - a.x) * 0.45) * Math.sign(b.x - a.x);
    return `M${a.x},${a.y} C${a.x + d},${a.y} ${b.x - d},${b.y} ${b.x},${b.y}`;
  }
  const d = Math.max(24, Math.abs(b.y - a.y) * 0.45) * Math.sign(b.y - a.y);
  return `M${a.x},${a.y} C${a.x},${a.y + d} ${b.x},${b.y - d} ${b.x},${b.y}`;
}

function Mesh({ selected, onSelect }: { selected: string | null; onSelect: (id: string) => void }) {
  const [ref, { w, h }] = useBox<HTMLDivElement>();
  const [hiddenKinds, setHiddenKinds] = useState<Set<string>>(new Set());
  const [hiddenRelations, setHiddenRelations] = useState<Set<string>>(new Set());
  const flip = (set: Set<string>, key: string) => {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  };

  const L = 230;
  const T = 64;
  const AW = Math.max(200, w - L - 28);
  const AH = Math.max(200, h - T - 28);
  const mx = AGENT_W / 2 + 26;
  const my = 40;
  const place = (u: Pt) => ({ x: L + mx + u.x * (AW - 2 * mx), y: T + my + u.y * (AH - 2 * my) });
  const pillW = (n: MeshNode) => clamp(n.label.length * 7.4 + 40, 84, 200);

  const rects: Record<string, Rect> = {};
  for (const n of mesh.nodes) {
    if (n.kind === "node") continue;
    const p = place(unit[n.id]);
    rects[n.id] =
      n.kind === "agent" ? { ...p, w: AGENT_W, h: AGENT_H } : { ...p, w: pillW(n), h: PILL_H };
  }
  const region = (ids: string[]): Rect => {
    const xs = ids.flatMap((i) => [rects[i].x - rects[i].w / 2, rects[i].x + rects[i].w / 2]);
    const ys = ids.flatMap((i) => [rects[i].y - rects[i].h / 2, rects[i].y + rects[i].h / 2]);
    const x0 = Math.min(...xs) - 22;
    const x1 = Math.max(...xs) + 22;
    const y0 = Math.min(...ys) - 40;
    const y1 = Math.max(...ys) + 22;
    return { x: (x0 + x1) / 2, y: (y0 + y1) / 2, w: x1 - x0, h: y1 - y0 };
  };
  const edgeCenter = place(edgeRegionUnit);
  rects["n-home"] = region(["a-researcher", "a-coder", "a-publisher"]);
  rects["n-lab"] = region(["a-verifier"]);
  rects["n-lab"] = { ...rects["n-lab"], w: Math.max(rects["n-lab"].w, 200), h: Math.max(rects["n-lab"].h, 130) };
  rects["n-edge"] = { x: edgeCenter.x, y: edgeCenter.y - 20, w: 200, h: 76 };

  const visible = (id: string) => !hiddenKinds.has(mesh.nodes.find((n) => n.id === id)!.kind);
  const neighbors = new Set(
    selected ? mesh.edges.filter((e) => e.from === selected || e.to === selected).flatMap((e) => [e.from, e.to]) : [],
  );
  const dim = (id: string) => selected !== null && id !== selected && !neighbors.has(id);
  const ready = w > 0;

  return (
    <div ref={ref} className="b-canvas b-canvas-mesh">
      <Filters
        hiddenKinds={hiddenKinds}
        hiddenRelations={hiddenRelations}
        toggleKind={(k) => setHiddenKinds((s) => flip(s, k))}
        toggleRelation={(r) => setHiddenRelations((s) => flip(s, r))}
      />
      <div className="b-mesh-head">
        <span className="b-kicker">
          フェデレーション全体 · <span className="mono">{mesh.nodes.length}</span> ノード ·{" "}
          <span className="mono">{mesh.edges.length}</span> 関係
        </span>
        <div className="b-toolgroup">
          <button type="button" className="b-ib" aria-label="縮小">
            <ZoomOut size={15} />
          </button>
          <span className="mono b-zoom">100%</span>
          <button type="button" className="b-ib" aria-label="拡大">
            <ZoomIn size={15} />
          </button>
          <span className="b-vsep" />
          <button type="button" className="b-ib" aria-label="全体を表示">
            <Maximize size={15} />
          </button>
        </div>
      </div>

      {ready && (
        <>
          {!hiddenKinds.has("node") &&
            mesh.nodes
              .filter((n) => n.kind === "node")
              .map((n) => {
                const r = rects[n.id];
                const peer = peers.find((p) => p.name === n.label);
                return (
                  <div
                    key={n.id}
                    className="b-zone"
                    data-tone={peer?.status === "degraded" ? "warn" : undefined}
                    data-selected={selected === n.id}
                    data-dim={dim(n.id)}
                    style={{ left: r.x - r.w / 2, top: r.y - r.h / 2, width: r.w, height: r.h }}
                  >
                    <button type="button" className="b-zone-label b-zone-btn" onClick={() => onSelect(n.id)}>
                      <span className="mono b-zone-id">aidash://{n.label}</span>
                      {peer ? (
                        <span className={peer.status === "degraded" ? "b-warn-text" : "b-faint"}>
                          {peer.status === "degraded" ? "劣化" : "接続"} <span className="mono">{peer.latencyMs}ms</span>
                        </span>
                      ) : (
                        <span className="b-faint">自ノード</span>
                      )}
                    </button>
                  </div>
                );
              })}

          <svg className="b-edges" width={w} height={h} aria-hidden>
            <defs>
              {[
                ["m-muted", "var(--b-edge-strong)"],
                ["m-accent", "var(--b-accent-mark)"],
                ["m-warn", "var(--b-warn)"],
                ["m-hi", "var(--b-text-2)"],
              ].map(([mid, color]) => (
                <marker key={mid} id={`b-${mid}`} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                  <path d="M0,0.5 L7.5,4 L0,7.5 z" style={{ fill: color }} />
                </marker>
              ))}
            </defs>
            {mesh.edges.map((e, i) => {
              if (hiddenRelations.has(e.relation) || !visible(e.from) || !visible(e.to)) return null;
              const { a, b, horizontal } = anchors(rects[e.from], rects[e.to]);
              const live = e.from === "a-coder" && e.to === "a-verifier";
              const hi = selected !== null && (e.from === selected || e.to === selected);
              const marker =
                e.relation === "depends" ? "m-warn" : live ? "m-accent" : hi ? "m-hi" : "m-muted";
              const d = path(a, b, horizontal);
              return (
                <g key={i} data-dim={selected !== null && !hi && e.relation !== "peer"}>
                  <path
                    d={d}
                    className="b-edge"
                    data-rel={e.relation}
                    data-live={live}
                    data-hi={hi}
                    markerEnd={e.relation === "handoff" || e.relation === "depends" ? `url(#b-${marker})` : undefined}
                  />
                  {live && <path d={d} className="b-edge-flow" />}
                </g>
              );
            })}
          </svg>

          {!hiddenKinds.has("node") &&
            !hiddenRelations.has("peer") &&
            mesh.edges
              .filter((e) => e.relation === "peer")
              .map((e) => {
                const { a, b } = anchors(rects[e.from], rects[e.to]);
                const peer = peers.find((p) => p.name === label(e.to))!;
                return (
                  <span
                    key={e.to}
                    className="b-elabel"
                    data-tone={peer.status === "degraded" ? "warn" : undefined}
                    style={{ left: (a.x + b.x) / 2, top: (a.y + b.y) / 2 }}
                  >
                    {peer.latencyMs}ms
                  </span>
                );
              })}

          {mesh.nodes.map((n) => {
            if (n.kind === "node" || hiddenKinds.has(n.kind)) return null;
            const r = rects[n.id];
            const agent = agentOf(n.id);
            if (agent) {
              const run = runs.find((x) => x.agent === agent.id)!;
              return (
                <button
                  key={n.id}
                  type="button"
                  className="b-mnode"
                  data-phase={agent.phase}
                  data-selected={selected === n.id}
                  data-dim={dim(n.id)}
                  aria-pressed={selected === n.id}
                  onClick={() => onSelect(n.id)}
                  style={{ left: r.x, top: r.y, width: r.w, height: r.h }}
                >
                  <Avatar id={agent.id} size={28} phase={agent.phase} />
                  <span className="min-w-0 text-left">
                    <span className="b-tile-name block">{agent.name}</span>
                    <span className="b-mnode-sub">
                      <Phase status={agent.phase} />
                      <span className="mono b-faint">s{run.step}</span>
                    </span>
                  </span>
                </button>
              );
            }
            return (
              <button
                key={n.id}
                type="button"
                className="b-pill b-pill-btn"
                data-kind={n.kind}
                data-selected={selected === n.id}
                data-dim={dim(n.id)}
                aria-pressed={selected === n.id}
                onClick={() => onSelect(n.id)}
                style={{ left: r.x, top: r.y, width: r.w }}
              >
                {n.kind === "tool" ? <Wrench size={12} /> : <Cpu size={12} />}
                <span className="mono">{n.label}</span>
              </button>
            );
          })}
        </>
      )}
    </div>
  );
}

export function ObserveScreen() {
  const [selected, setSelected] = useState<string | null>("a-verifier");
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1">
        <Mesh selected={selected} onSelect={(id) => setSelected(id === selected ? null : id)} />
        {selected && <Inspector id={selected} onClose={() => setSelected(null)} />}
      </div>
      <Timeline expanded />
    </div>
  );
}
