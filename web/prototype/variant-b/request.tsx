// PROTOTYPE ONLY. Variant B request screen: live canvas of one request + dock + timeline strip.
import { useState, type CSSProperties } from "react";
import {
  ArrowRight,
  AtSign,
  Download,
  ExternalLink,
  FileDiff,
  FileText,
  Layers,
  Maximize,
  MessagesSquare,
  PanelRightClose,
  PanelRightOpen,
  Paperclip,
  SendHorizontal,
  SquareCheck,
  Wrench,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  agentName,
  agents,
  artifacts,
  events,
  humanRequest,
  messages,
  peers,
  runs,
  statusLabel,
  tasks,
  workspaces,
} from "../data";
import { DecisionCard } from "./shell";
import { Avatar, Phase, Timeline, clamp, useBox, type Decision } from "./shared";

type Tab = "chat" | "tasks" | "artifacts";
type Box = { x: number; y: number; w: number; h: number };
type Pt = { x: number; y: number };

const ws = workspaces[0];
const TILE_H = 132;
const lab = peers.find((p) => p.name === "lab")!;
const edge = peers.find((p) => p.name === "edge-02")!;
const lastChaos = [...events].reverse().find((e) => e.text.startsWith("chaos.disconnect"))!;

// Horizontal-tangent bezier between two anchor points.
const curve = (a: Pt, b: Pt) => {
  const dx = Math.max(40, Math.abs(b.x - a.x) * 0.5);
  const sa = b.x >= a.x ? 1 : -1;
  return `M${a.x},${a.y} C${a.x + dx * sa},${a.y} ${b.x - dx * sa},${b.y} ${b.x},${b.y}`;
};

function AgentTile({
  id,
  center,
  width,
  selected,
  onSelect,
}: {
  id: string;
  center: Pt;
  width: number;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  const agent = agents.find((a) => a.id === id)!;
  const run = runs.find((r) => r.agent === id)!;
  const task = tasks.find((t) => t.id === run.task)!;
  return (
    <button
      type="button"
      className="b-tile"
      data-phase={agent.phase}
      data-selected={selected}
      aria-pressed={selected}
      onClick={() => onSelect(id)}
      style={{ left: center.x, top: center.y, width, height: TILE_H } as CSSProperties}
    >
      <span className="flex items-start gap-2">
        <Avatar id={id} phase={agent.phase} size={28} />
        <span className="min-w-0 flex-1">
          <span className="flex items-center justify-between gap-2">
            <span className="b-tile-name">{agent.name}</span>
            <span className="b-nodebadge mono">{agent.node.replace("aidash://", "")}</span>
          </span>
          <span className="b-tile-model mono">{agent.model}</span>
        </span>
      </span>
      <span className="b-tile-rule" />
      <span className="flex items-center justify-between gap-2">
        <Phase status={agent.phase} />
        <span className="mono b-faint">
          {task.id} · s{run.step}
        </span>
      </span>
      <span className="b-tile-task">{task.title}</span>
    </button>
  );
}

function RequestCanvas({
  decision,
  setDecision,
  dockOpen,
  openDock,
}: {
  decision: Decision;
  setDecision: (d: Decision) => void;
  dockOpen: boolean;
  openDock: () => void;
}) {
  const [ref, { w, h }] = useBox<HTMLDivElement>();
  const [selected, setSelected] = useState("verifier");

  const L = 20;
  const T = 92;
  const AW = w - 40;
  const AH = h - T - 20;
  const tileW = clamp(AW * 0.21, 172, 210);
  const row0 = T + 48 + TILE_H / 2;
  const homeW = AW * 0.6;
  const pubY = row0 + TILE_H + clamp(AH * 0.2, 90, 150);
  // The decision card (~250px tall) is centred on Publisher, so it sets the home zone's depth.
  const contentBottom = Math.min(T + AH, pubY + 153);
  const chaosY = row0 + TILE_H / 2 + 56;
  const labBottom = chaosY + 34;
  const zones: Record<"home" | "lab" | "edge", Box> = {
    home: { x: L, y: T, w: homeW, h: contentBottom - T },
    lab: { x: L + AW * 0.64, y: T, w: AW * 0.36, h: labBottom - T },
    edge: { x: L + AW * 0.64, y: labBottom + 24, w: AW * 0.36, h: contentBottom - labBottom - 24 },
  };
  const home = zones.home;
  const pos: Record<string, Pt> = {
    researcher: { x: home.x + 24 + tileW / 2, y: row0 },
    coder: { x: home.x + home.w - 24 - tileW / 2, y: row0 },
    publisher: { x: home.x + home.w - 24 - tileW / 2, y: pubY },
    verifier: { x: zones.lab.x + zones.lab.w / 2, y: row0 },
  };
  const chaos: Pt = { x: zones.lab.x + zones.lab.w / 2, y: chaosY };
  const edgeTarget: Pt = { x: zones.edge.x + zones.edge.w / 2, y: zones.edge.y + 1 };
  const right = (id: string, dy = 0) => ({ x: pos[id].x + tileW / 2, y: pos[id].y + dy });
  const left = (id: string, dy = 0) => ({ x: pos[id].x - tileW / 2, y: pos[id].y + dy });
  const cardLeft = home.x + 24;
  const cardW = clamp(pos.publisher.x - tileW / 2 - 40 - cardLeft, 200, 300);
  const cardRight = cardLeft + cardW;

  const ready = w > 0;
  const mid = (a: Pt, b: Pt) => ({ left: (a.x + b.x) / 2, top: (a.y + b.y) / 2 });

  const vToP = { a: left("verifier", 40), b: right("publisher") };
  const done = tasks.filter((t) => t.status === "COMPLETED").length;

  return (
    <div ref={ref} className="b-canvas">
      <header className="b-canvas-head">
        <div className="min-w-0">
          <div className="b-kicker">
            <span className="mono">{ws.id}</span>
            <span className="b-faint">·</span>
            <Phase status={ws.status} />
          </div>
          <h1 className="b-title mono">{ws.title}</h1>
          <p className="b-goal">{ws.goal}</p>
        </div>
        <div className="b-canvas-tools">
          <div className="b-progress" aria-label={`タスク ${done}/${tasks.length}`}>
            <span className="b-muted">タスク</span>
            <span className="mono">
              {done}/{tasks.length}
            </span>
            <span className="b-segbar">
              {tasks.map((t) => (
                <span key={t.id} data-status={t.status} title={`${t.id} ${statusLabel[t.status]}`} />
              ))}
            </span>
          </div>
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
            <button type="button" className="b-ib" aria-label="レイヤー">
              <Layers size={15} />
            </button>
          </div>
          {!dockOpen && (
            <button type="button" className="b-btn" onClick={openDock}>
              <PanelRightOpen size={14} />
              会話を開く
            </button>
          )}
        </div>
      </header>

      {ready && (
        <>
          {(Object.keys(zones) as (keyof typeof zones)[]).map((k) => {
            const z = zones[k];
            return (
              <div
                key={k}
                className="b-zone"
                data-tone={k === "edge" ? "warn" : undefined}
                style={{ left: z.x, top: z.y, width: z.w, height: z.h }}
              >
                <div className="b-zone-label">
                  <span className="mono b-zone-id">aidash://{k === "edge" ? "edge-02" : k}</span>
                  {k === "home" && <span className="b-faint">自ノード · 3 エージェント</span>}
                  {k === "lab" && (
                    <span className="b-faint">
                      接続 <span className="mono">{lab.latencyMs}ms</span> · 1 エージェント
                    </span>
                  )}
                  {k === "edge" && (
                    <span className="b-warn-text">
                      劣化 <span className="mono">{edge.latencyMs}ms</span>
                    </span>
                  )}
                </div>
              </div>
            );
          })}

          <svg className="b-edges" width={w} height={h} aria-hidden>
            <defs>
              {[
                ["muted", "var(--b-edge)"],
                ["accent", "var(--b-accent-mark)"],
                ["warn", "var(--b-warn)"],
              ].map(([id, color]) => (
                <marker key={id} id={`b-arrow-${id}`} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                  <path d="M0,0.5 L7.5,4 L0,7.5 z" style={{ fill: color }} />
                </marker>
              ))}
            </defs>
            <path d={curve(right("researcher"), left("coder"))} className="b-edge" markerEnd="url(#b-arrow-muted)" />
            <path d={curve(right("coder"), left("verifier"))} className="b-edge b-edge-live" markerEnd="url(#b-arrow-accent)" />
            <path d={curve(right("coder"), left("verifier"))} className="b-edge-flow" />
            <path d={curve(vToP.a, vToP.b)} className="b-edge b-edge-wait" markerEnd="url(#b-arrow-warn)" />
            <path
              d={`M${pos.verifier.x},${pos.verifier.y + TILE_H / 2} L${chaos.x},${chaos.y - 14}`}
              className="b-edge b-edge-thin"
            />
            <path d={`M${chaos.x},${chaos.y + 14} L${edgeTarget.x},${edgeTarget.y + 40}`} className="b-edge b-edge-hit" markerEnd="url(#b-arrow-warn)" />
            {decision === "pending" && (
              <path
                d={`M${cardRight},${pos.publisher.y} L${pos.publisher.x - tileW / 2},${pos.publisher.y}`}
                className="b-edge b-edge-tether"
              />
            )}
          </svg>

          <span className="b-elabel" style={mid(right("researcher"), left("coder"))}>
            → task-3
          </span>
          <span className="b-elabel" data-tone="live" style={mid(right("coder"), left("verifier"))}>
            → task-4
          </span>
          <span className="b-elabel" data-tone="warn" style={mid(vToP.a, vToP.b)}>
            task-4 の完了待ち
          </span>

          {agents.map((a) => (
            <AgentTile
              key={a.id}
              id={a.id}
              center={pos[a.id]}
              width={tileW}
              selected={selected === a.id}
              onSelect={setSelected}
            />
          ))}

          <span className="b-pill" style={{ left: chaos.x, top: chaos.y }}>
            <Wrench size={12} />
            <span className="mono">chaos</span>
          </span>
          <div className="b-edge-target" style={{ left: edgeTarget.x, top: edgeTarget.y + 40 }}>
            <span className="mono">edge-02</span>
            <span className="b-faint">
              切断テスト 3 回目 · <span className="mono">{lastChaos.at.slice(0, 5)}</span>
            </span>
          </div>

          <DecisionCard
            decision={decision}
            setDecision={setDecision}
            className="b-decision-anchored"
            style={{ left: cardLeft, top: pos.publisher.y, width: cardW }}
          />
        </>
      )}

      <div className="b-legend">
        <span>
          <i className="b-lg-line" /> 引き継ぎ
        </span>
        <span>
          <i className="b-lg-line" data-tone="live" /> 実行中
        </span>
        <span>
          <i className="b-lg-line" data-tone="warn" /> 依存・待機
        </span>
        <span>
          <span className="b-kbd">Space</span> + ドラッグで移動
        </span>
      </div>
    </div>
  );
}

function Chat({ decision }: { decision: Decision }) {
  return (
    <div className="b-dock-scroll">
      <div className="b-daymark mono">2026-10-10</div>
      {messages.map((m) =>
        m.sender === "system" ? (
          <div key={m.id} className="b-sysmsg">
            <span className="mono b-faint">{m.at}</span>
            <span>{m.content}</span>
          </div>
        ) : (
          <article key={m.id} className="b-msg" data-who={m.sender === "human" ? "human" : "agent"}>
            <Avatar id={m.sender} size={24} />
            <div className="min-w-0">
              <header className="b-msg-head">
                <span className="b-msg-name">{agentName(m.sender)}</span>
                <span className="mono b-faint">{m.at}</span>
              </header>
              <p className="b-msg-body">{m.content}</p>
              {(m.attachment || m.thread) && (
                <div className="mt-1.5 flex flex-wrap gap-1.5">
                  {m.attachment && (
                    <button type="button" className="b-chip">
                      {m.attachment.endsWith(".diff") ? <FileDiff size={12} /> : <FileText size={12} />}
                      <span className="mono">{m.attachment}</span>
                    </button>
                  )}
                  {m.thread && (
                    <button type="button" className="b-chip">
                      <MessagesSquare size={12} />
                      返信 <span className="mono">{m.thread}</span>
                    </button>
                  )}
                </div>
              )}
            </div>
          </article>
        ),
      )}
      <div className="b-inline-decision" data-decision={decision}>
        <span className="b-dot" />
        <span className="flex-1">
          {decision === "pending" ? "判断待ち: " : decision === "approved" ? "承認済み: " : "却下済み: "}
          {humanRequest.prompt.replace("を承認しますか？", "")}
        </span>
        <span className="mono b-faint">{humanRequest.taskId}</span>
      </div>
      <div className="b-typing">
        <Avatar id="verifier" size={18} phase="THINKING" />
        <span>Verifier が思考中</span>
        <span className="b-typing-dots" aria-hidden>
          <i />
          <i />
          <i />
        </span>
        <span className="mono b-faint ml-auto">step 9</span>
      </div>
    </div>
  );
}

function Composer() {
  const [text, setText] = useState("");
  return (
    <form className="b-composer" onSubmit={(e) => e.preventDefault()}>
      <div className="b-composer-to">
        <span className="b-faint">宛先</span>
        <button type="button" className="b-chip">
          全エージェント
        </button>
        <span className="b-faint ml-auto">
          <span className="b-kbd">⌘</span> <span className="b-kbd">Enter</span> で送信
        </span>
      </div>
      <textarea
        className="b-input"
        rows={3}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="指示を送る。@ でエージェントを指定"
        aria-label="メッセージ"
      />
      <div className="flex items-center gap-1">
        <button type="button" className="b-ib" aria-label="メンション">
          <AtSign size={15} />
        </button>
        <button type="button" className="b-ib" aria-label="ファイルを添付">
          <Paperclip size={15} />
        </button>
        <button type="submit" className="b-btn b-btn-primary ml-auto" disabled={!text.trim()}>
          送信
          <SendHorizontal size={14} />
        </button>
      </div>
    </form>
  );
}

const taskGroups = [
  { label: "実行中", statuses: ["RUNNING"] },
  { label: "保留", statuses: ["BLOCKED"] },
  { label: "完了", statuses: ["COMPLETED"] },
];

function Tasks() {
  return (
    <div className="b-dock-scroll">
      {taskGroups.map((g) => {
        const list = tasks.filter((t) => g.statuses.includes(t.status));
        return (
          <section key={g.label} className="b-group">
            <h3 className="b-group-head">
              {g.label} <span className="mono b-faint">{list.length}</span>
            </h3>
            {list.map((t) => (
              <div key={t.id} className="b-task">
                <Phase status={t.status}>
                  <span className="mono">{t.id}</span>
                </Phase>
                <div className="min-w-0">
                  <div className="b-task-title">{t.title}</div>
                  <div className="b-task-meta">
                    <span>{agentName(t.owner)}</span>
                    <span className="mono">
                      {t.startedAt} → {t.finishedAt ?? "実行中"}
                    </span>
                    {t.dependencies.length > 0 && (
                      <span className="mono b-faint">依存 {t.dependencies.join(", ")}</span>
                    )}
                  </div>
                </div>
                <span className="mono b-faint">s{t.step}</span>
              </div>
            ))}
          </section>
        );
      })}
    </div>
  );
}

function Artifacts() {
  return (
    <div className="b-dock-scroll">
      {artifacts.map((a) => (
        <article key={a.id} className="b-artifact">
          <header className="flex items-center gap-2.5">
            <span className="b-file">{a.kind === "patch" ? <FileDiff size={15} /> : <FileText size={15} />}</span>
            <span className="min-w-0 flex-1">
              <span className="mono block b-artifact-name">{a.name}</span>
              <span className="b-faint">
                {agentName(a.by)} · <span className="mono">{a.createdAt} · {a.size}</span>
              </span>
            </span>
            <button type="button" className="b-ib" aria-label="開く">
              <ExternalLink size={14} />
            </button>
            <button type="button" className="b-ib" aria-label="ダウンロード">
              <Download size={14} />
            </button>
          </header>
          <ul className="b-preview mono">
            {a.preview.map((line) => (
              <li key={line}>
                {a.kind === "document" ? <SquareCheck size={12} className="b-faint" /> : <ArrowRight size={12} className="b-faint" />}
                {line}
              </li>
            ))}
          </ul>
        </article>
      ))}
      <p className="b-dock-note">
        Publisher の公開が承認されると、<span className="mono">aidash-runtime 0.1.0</span> のパッケージが追加されます。
      </p>
    </div>
  );
}

export function RequestScreen({ decision, setDecision }: { decision: Decision; setDecision: (d: Decision) => void }) {
  const [dockOpen, setDockOpen] = useState(true);
  const [tab, setTab] = useState<Tab>("chat");
  const tabs: { key: Tab; label: string; count: number }[] = [
    { key: "chat", label: "会話", count: messages.length },
    { key: "tasks", label: "タスク", count: tasks.length },
    { key: "artifacts", label: "成果物", count: artifacts.length },
  ];
  return (
    <div className="flex h-full min-h-0">
      <div className="flex min-w-0 flex-1 flex-col">
        <RequestCanvas decision={decision} setDecision={setDecision} dockOpen={dockOpen} openDock={() => setDockOpen(true)} />
        <Timeline />
      </div>
      {dockOpen && (
        <aside className="b-dock" aria-label="依頼の詳細">
          <div className="b-tabs" role="tablist">
            {tabs.map((t) => (
              <button
                key={t.key}
                type="button"
                role="tab"
                className="b-tab"
                aria-selected={tab === t.key}
                onClick={() => setTab(t.key)}
              >
                {t.label}
                <span className="mono b-faint">{t.count}</span>
              </button>
            ))}
            <button type="button" className="b-ib ml-auto self-center" aria-label="ドックを閉じる" onClick={() => setDockOpen(false)}>
              <PanelRightClose size={15} />
            </button>
          </div>
          {tab === "chat" && <Chat decision={decision} />}
          {tab === "tasks" && <Tasks />}
          {tab === "artifacts" && <Artifacts />}
          {tab === "chat" && <Composer />}
        </aside>
      )}
    </div>
  );
}
