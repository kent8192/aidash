// PROTOTYPE ONLY. Variant C observe screen: an execution report written as a document.
import { Download, Ellipsis } from "lucide-react";
import { agentName, agents, events, mesh, peers, runs, tasks, workspaces, type EventKind } from "../data";
import type { Screen } from "../types";
import { Avatar, NOW, StatusWord, TopStrip, nodeName } from "./parts";

const workspace = workspaces[0];

const kindWord: Record<EventKind, string> = {
  task: "タスク",
  run: "実行",
  tool: "ツール",
  request: "依頼",
  artifact: "成果物",
  peer: "ピア",
  error: "エラー",
};

// ---- Derived report facts ----
const done = tasks.filter((t) => t.status === "COMPLETED").length;
const runningTask = tasks.find((t) => t.status === "RUNNING")!;
const runningAgent = agents.find((a) => a.id === runningTask.owner)!;
const blockedTask = tasks.find((t) => t.status === "BLOCKED")!;
const degraded = peers.find((p) => p.status === "degraded")!;
const warnings = events.filter((e) => e.severity === "warn");
const totals = runs.reduce(
  (sum, r) => ({
    step: sum.step + r.step,
    tokensIn: sum.tokensIn + r.tokensIn,
    tokensOut: sum.tokensOut + r.tokensOut,
    toolCalls: sum.toolCalls + r.toolCalls,
  }),
  { step: 0, tokensIn: 0, tokensOut: 0, toolCalls: 0 },
);
const fmt = new Intl.NumberFormat("ja-JP");

// Events grouped by actor, ordered by first appearance.
const groups = events.reduce<{ actor: string; items: typeof events }[]>((acc, event) => {
  const group = acc.find((g) => g.actor === event.actor);
  if (group) group.items.push(event);
  else acc.push({ actor: event.actor, items: [event] });
  return acc;
}, []);

export function ObserveScreen({ onScreen }: { onScreen: (s: Screen) => void }) {
  return (
    <>
      <TopStrip
        crumbs={[{ label: "観測" }, { label: workspace.title, onClick: () => onScreen("request") }, { label: "実行レポート" }]}
        note={`${NOW} 時点`}
      >
        <button type="button" className="c-btn c-btn--sm">
          <Download size={14} />
          書き出す
        </button>
        <button type="button" className="c-icon-btn" aria-label="その他の操作">
          <Ellipsis size={16} />
        </button>
      </TopStrip>

      <div className="c-page">
        <article className="c-doc">
          <div className="c-eyebrow">
            <button type="button" className="c-link" onClick={() => onScreen("request")}>
              {workspace.title}
            </button>
            <span aria-hidden>·</span>
            <span>自動生成</span>
          </div>
          <h1 className="c-title">実行レポート</h1>
          <p className="c-lede num">
            {tasks.length} タスク中 {done} 件が完了しました。{runningAgent.name} が {nodeName(runningAgent.node)}{" "}
            ノードで「{runningTask.title}」を実行中で、edge-02 の切断から 3 回中 2 回、14 秒以内に回復しています。
            {agentName(blockedTask.owner)} は「{blockedTask.title}」の承認を待っています。
          </p>
          <dl className="c-meta">
            <div>
              <dt>期間</dt>
              <dd className="mono">09:01–{NOW}</dd>
            </div>
            <div>
              <dt>実行</dt>
              <dd className="num">{runs.length}</dd>
            </div>
            <div>
              <dt>トークン</dt>
              <dd className="num">
                {fmt.format(totals.tokensIn)} / {fmt.format(totals.tokensOut)}
              </dd>
            </div>
            <div>
              <dt>ツール呼び出し</dt>
              <dd className="num">{totals.toolCalls}</dd>
            </div>
          </dl>

          <h2 className="c-h2" id="c-obs-summary">
            注意が必要な点
            <small className="num">{warnings.length} 件</small>
          </h2>
          <dl className="c-log">
            {warnings.map((event) => (
              <div key={event.at} className="contents">
                <dt>{event.at.slice(0, 5)}</dt>
                <dd>
                  <strong>{agentName(event.actor)}</strong> {event.text}
                </dd>
              </div>
            ))}
          </dl>
          <p className="c-p mt-3 text-[14.5px] text-[var(--c-ink-2)]">
            {degraded.name} の応答遅延は {degraded.latencyMs}ms で、ほかのピアより大きく遅れています。復旧テストの判定はこの遅延を含めた結果として読む必要があります。
          </p>

          <h2 className="c-h2" id="c-obs-mesh">
            メッシュ
          </h2>
          <MeshFigure />

          <h2 className="c-h2" id="c-obs-timeline">
            経過
            <small className="num">{events.length} 件のイベント</small>
          </h2>
          <div>
            {groups.map((group) => {
              const agent = agents.find((a) => a.id === group.actor);
              return (
                <section key={group.actor} className="c-tl-group">
                  <div className="c-tl-who">
                    <strong>
                      <Avatar id={group.actor} />
                      {agentName(group.actor)}
                    </strong>
                    <div className="c-tl-who__sub">
                      {agent ? (
                        <>
                          {nodeName(agent.node)}
                          <br />
                          <span className="mono">{agent.model}</span>
                        </>
                      ) : group.actor === "human" ? (
                        "依頼者"
                      ) : (
                        "システム"
                      )}
                    </div>
                  </div>
                  <ol className="c-tl-list">
                    {group.items.map((event) => (
                      <li key={event.at} className={`c-tl-item${event.severity === "warn" ? " c-tl-item--warn" : ""}`}>
                        <time>{event.at}</time>
                        <span className="c-tl-kind">{kindWord[event.kind]}</span>
                        <span className={event.severity === "warn" ? "text-[var(--c-ink)]" : "text-[var(--c-ink-2)]"}>
                          {event.text}
                          {event.severity === "warn" && <span className="tone-warn ml-2 text-[12.5px]">注意</span>}
                        </span>
                      </li>
                    ))}
                  </ol>
                </section>
              );
            })}
          </div>

          <h2 className="c-h2" id="c-obs-runs">
            実行
            <small className="num">{runs.length} 件</small>
          </h2>
          <table className="c-table">
            <thead>
              <tr>
                <th>エージェント</th>
                <th>モデル</th>
                <th className="r">ステップ</th>
                <th className="r">入力</th>
                <th className="r">出力</th>
                <th className="r">ツール</th>
                <th className="r">所要</th>
              </tr>
            </thead>
            <tbody>
              {runs.map((run) => {
                const agent = agents.find((a) => a.id === run.agent)!;
                return (
                  <tr key={run.id}>
                    <td>
                      <div className="font-medium">{agent.name}</div>
                      <div className="text-[12px] text-[var(--c-muted)]">
                        <span className="mono">{run.id}</span> · <StatusWord status={run.phase} />
                      </div>
                    </td>
                    <td className="mono text-[var(--c-ink-2)]">{agent.model}</td>
                    <td className="r">{run.step}</td>
                    <td className="r">{fmt.format(run.tokensIn)}</td>
                    <td className="r">{fmt.format(run.tokensOut)}</td>
                    <td className="r">{run.toolCalls}</td>
                    <td className="r mono">{run.duration}</td>
                  </tr>
                );
              })}
            </tbody>
            <tfoot>
              <tr>
                <td className="pt-3">合計</td>
                <td />
                <td className="r pt-3">{totals.step}</td>
                <td className="r pt-3">{fmt.format(totals.tokensIn)}</td>
                <td className="r pt-3">{fmt.format(totals.tokensOut)}</td>
                <td className="r pt-3">{totals.toolCalls}</td>
                <td />
              </tr>
            </tfoot>
          </table>
        </article>

        <aside className="c-aside" aria-label="このレポートについて">
          <div className="c-aside__block">
            <div className="c-aside__label">このレポート</div>
            <dl>
              <dt>対象</dt>
              <dd>
                <button type="button" className="c-link" onClick={() => onScreen("request")}>
                  {workspace.title}
                </button>
              </dd>
              <dt>生成</dt>
              <dd className="mono">{NOW} 自動</dd>
              <dt>ノード</dt>
              <dd>home, {peers.map((p) => p.name).join(", ")}</dd>
            </dl>
          </div>
          <div className="c-aside__block">
            <div className="c-aside__label">目次</div>
            <nav className="c-toc">
              <a href="#c-obs-summary">注意が必要な点</a>
              <a href="#c-obs-mesh">メッシュ</a>
              <a href="#c-obs-timeline">経過</a>
              <a href="#c-obs-runs">実行</a>
            </nav>
          </div>
          <div className="c-aside__block">
            <div className="c-aside__label">エージェント</div>
            {agents.map((agent) => (
              <div key={agent.id} className="c-agentline">
                <Avatar id={agent.id} />
                <span>
                  {agent.name} <span className="muted text-[12px]">{nodeName(agent.node)}</span>
                </span>
                <span className="text-[12.5px]">
                  <StatusWord status={agent.phase} />
                </span>
              </div>
            ))}
          </div>
        </aside>
      </div>
    </>
  );
}

// ---- Static mesh figure (data-driven SVG) ----

type Box = { id: string; x: number; y: number };
const boxW = 132;
const boxH = 64;
const agentBoxes: Box[] = [
  { id: "researcher", x: 20, y: 66 },
  { id: "coder", x: 222, y: 66 },
  { id: "publisher", x: 222, y: 166 },
  { id: "verifier", x: 414, y: 66 },
];
const regions = [
  { id: "n-home", x: 0.5, w: 371.5, latency: null as number | null },
  { id: "n-lab", x: 392, w: 176, latency: peers[0].latencyMs },
  { id: "n-edge", x: 588, w: 131.5, latency: peers[1].latencyMs },
];

function MeshFigure() {
  const box = (agentId: string) => agentBoxes.find((b) => b.id === agentId)!;
  const handoffs = mesh.edges.filter((e) => e.relation === "handoff");
  const depends = mesh.edges.find((e) => e.relation === "depends")!;
  const dependsFrom = box(depends.from.slice(2));
  const dependsTo = box(depends.to.slice(2));
  const ink2 = "#3b433e";
  const rule = "#d3d6cf";
  const muted = "#626a64";

  return (
    <figure className="c-figure">
      <svg viewBox="0 0 720 316" role="img" aria-label="エージェントメッシュの図">
        <defs>
          <marker id="c-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M0 0.5 7 4 0 7.5z" fill={ink2} />
          </marker>
        </defs>

        {regions.map((r) => (
          <g key={r.id}>
            <rect x={r.x} y={18} width={r.w} height={232} rx={4} fill="#f6f7f4" stroke="#e3e5df" />
            <text x={r.x + 14} y={42} fontSize={13} fontWeight={700} fill="#1c211e">
              {mesh.nodes.find((n) => n.id === r.id)!.label}
            </text>
            <text x={r.x + 14} y={57} fontSize={10.5} fill={muted} fontFamily="IBM Plex Mono, monospace">
              {r.latency === null ? "このノード" : `${r.latency}ms`}
            </text>
          </g>
        ))}

        {/* edge-02 hosts no agents in this request */}
        <text x={602} y={140} fontSize={12} fill={muted}>
          エージェントなし
        </text>
        <text x={602} y={160} fontSize={12} fill="#9a6a12">
          劣化中
        </text>

        {/* Handoff edges */}
        {handoffs.map((edge) => {
          const from = box(edge.from.slice(2));
          const to = box(edge.to.slice(2));
          const y = from.y + 30;
          return (
            <g key={`${edge.from}-${edge.to}`}>
              <line x1={from.x + boxW} y1={y} x2={to.x - 3} y2={y} stroke={ink2} strokeWidth={1.25} markerEnd="url(#c-arrow)" />
              <text x={(from.x + boxW + to.x) / 2} y={y - 7} fontSize={10.5} fill={muted} textAnchor="middle">
                引き継ぎ
              </text>
            </g>
          );
        })}

        {/* Dependency: Publisher waits on Verifier */}
        <path
          d={`M${dependsFrom.x + boxW - 30} ${dependsFrom.y + boxH} V${dependsTo.y + 32} H${dependsTo.x + boxW + 3}`}
          fill="none"
          stroke={ink2}
          strokeWidth={1.25}
          strokeDasharray="4 4"
          markerEnd="url(#c-arrow)"
        />
        <text x={dependsFrom.x + boxW - 22} y={dependsTo.y + 24} fontSize={10.5} fill={muted}>
          依存
        </text>

        {agentBoxes.map((b) => {
          const agent = agents.find((a) => a.id === b.id)!;
          const active = agent.phase === "THINKING";
          const waiting = agent.phase === "WAITING";
          const tools = mesh.edges
            .filter((e) => e.from === `a-${b.id}` && e.relation === "uses")
            .map((e) => mesh.nodes.find((n) => n.id === e.to)!.label);
          return (
            <g key={b.id}>
              <rect
                x={b.x}
                y={b.y}
                width={boxW}
                height={boxH}
                rx={4}
                fill="#fcfcfb"
                stroke={active ? "#0f6b66" : rule}
                strokeWidth={active ? 1.5 : 1}
              />
              <text x={b.x + 12} y={b.y + 22} fontSize={13} fontWeight={700} fill="#1c211e">
                {agent.name}
              </text>
              <text x={b.x + 12} y={b.y + 38} fontSize={10.5} fill={muted} fontFamily="IBM Plex Mono, monospace">
                {agent.model}
              </text>
              <text
                x={b.x + 12}
                y={b.y + 55}
                fontSize={11}
                fill={active ? "#0f6b66" : waiting ? "#9a6a12" : muted}
                fontWeight={active || waiting ? 500 : 400}
              >
                {active ? "思考中" : waiting ? "承認待ち" : "完了"}
              </text>
              {tools.length > 0 && (
                <text x={b.x + 12} y={b.y + boxH + 17} fontSize={10.5} fill={muted} fontFamily="IBM Plex Mono, monospace">
                  tool: {tools.join(", ")}
                </text>
              )}
            </g>
          );
        })}

        {/* Federation peer links */}
        {regions.map((r, index) => {
          const x = r.x + r.w / 2;
          const next = regions[index + 1];
          const peer = next && peers.find((p) => p.id === `aidash://${mesh.nodes.find((n) => n.id === next.id)!.label}`)!;
          const tone = peer?.status === "degraded" ? "#9a6a12" : ink2;
          const nextX = next ? next.x + next.w / 2 : x;
          return (
            <g key={`peer-${r.id}`}>
              <line x1={x} y1={250} x2={x} y2={286} stroke={rule} />
              {peer && (
                <>
                  <line
                    x1={x}
                    y1={286}
                    x2={nextX}
                    y2={286}
                    stroke={tone}
                    strokeWidth={1.25}
                    strokeDasharray={peer.status === "degraded" ? "3 3" : undefined}
                  />
                  <text
                    x={(x + nextX) / 2}
                    y={305}
                    fontSize={10.5}
                    fill={peer.status === "degraded" ? tone : muted}
                    textAnchor="middle"
                    fontFamily="IBM Plex Mono, monospace"
                  >
                    peer · {peer.latencyMs}ms
                  </text>
                </>
              )}
              <rect x={x - 3} y={283} width={6} height={6} rx={1} fill={r.id === "n-edge" ? "#9a6a12" : ink2} />
            </g>
          );
        })}
      </svg>
      <figcaption>
        <b>図 1</b>
        {NOW} 時点のエージェントメッシュ。実線の矢印は引き継ぎ、破線は依存を表します。下段はノード間のピア接続で、edge-02
        への接続は応答遅延により劣化しています。
      </figcaption>
    </figure>
  );
}
