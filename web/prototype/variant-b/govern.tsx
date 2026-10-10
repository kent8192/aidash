// PROTOTYPE ONLY. Variant B govern screen: metrics, policy x agent matrix, incidents, audit log.
import { useState } from "react";
import { Download, Search } from "lucide-react";
import { agentName, agents, audit, incidents, mesh, policies, trustMetrics } from "../data";
import { Avatar, type Decision } from "./shared";

type Policy = (typeof policies)[number];
type Effect = Policy["effect"];

const effectLabel: Record<string, string> = {
  allow: "許可",
  deny: "拒否",
  "require-approval": "承認",
  limit: "上限",
};
const decisionLabel: Record<string, string> = { allow: "許可", deny: "拒否", pending: "判断待ち" };

// A policy applies to an agent when its scope names the agent, its tenant, its role, or a tool it uses.
function applies(scope: string, agentId: string): string | null {
  const [kind, value] = scope.split(":");
  if (kind === "agent") return value === agentId ? "対象エージェント" : null;
  if (kind === "tenant") return "テナント全体";
  if (kind === "role") return value === "agent" ? "ロール agent" : null;
  if (kind === "tool")
    return mesh.edges.some((e) => e.from === `a-${agentId}` && e.to === `t-${value}`) ? `${value} を利用` : null;
  return null;
}

const tones: Record<string, string> = { allow: "done", deny: "error", pending: "warn" };

function Metrics({ decision }: { decision: Decision }) {
  const counts = { allow: 0, deny: 0, pending: 0 } as Record<string, number>;
  for (const a of audit) counts[a.decision] += 1;
  return (
    <section className="b-metrics" aria-label="指標">
      {trustMetrics.map((m) => {
        const value = m.label === "承認待ち" && decision !== "pending" ? "0" : m.value;
        const tone = m.label === "承認待ち" ? "warn" : m.label === "未解決インシデント" ? "error" : undefined;
        return (
          <div key={m.label} className="b-metric">
            <span className="b-metric-label">
              {tone && <span className="b-dot" data-tone={tone} />}
              {m.label}
            </span>
            <span className="b-bignum">{value}</span>
            <span className="b-faint">{m.delta}</span>
          </div>
        );
      })}
      <div className="b-metric">
        <span className="b-metric-label">判定の内訳</span>
        <span className="b-split" aria-hidden>
          {(["allow", "deny", "pending"] as const).map((k) => (
            <span key={k} data-tone={tones[k]} style={{ flexGrow: counts[k] }} />
          ))}
        </span>
        <span className="b-split-legend">
          {(["allow", "deny", "pending"] as const).map((k) => (
            <span key={k}>
              <span className="b-dot" data-tone={tones[k]} />
              {decisionLabel[k]} <span className="mono">{counts[k]}</span>
            </span>
          ))}
        </span>
      </div>
    </section>
  );
}

function Matrix() {
  const [cell, setCell] = useState<[string, string]>(["pol-net", "coder"]);
  const policy = policies.find((p) => p.id === cell[0])!;
  const reason = applies(policy.scope, cell[1]);
  const hits = audit.filter((a) => a.policy === cell[0] && a.actor === cell[1]);
  return (
    <section className="b-sec">
      <header className="b-sec-head">
        <h2>ポリシー × エージェント</h2>
        <span className="b-faint">セルの数字は直近の監査ヒット数</span>
        <span className="b-matrix-legend">
          {(Object.keys(effectLabel) as Effect[]).map((e) => (
            <span key={e}>
              <i className="b-swatch" data-effect={e} />
              {effectLabel[e]}
            </span>
          ))}
          <span>
            <i className="b-swatch" />
            適用外
          </span>
        </span>
      </header>
      <div className="b-matrix" role="grid" style={{ gridTemplateColumns: `minmax(220px, 1.5fr) repeat(${agents.length}, minmax(96px, 1fr))` }}>
        <span className="b-mx-corner b-faint">ポリシー</span>
        {agents.map((a) => (
          <span key={a.id} className="b-mx-col">
            <Avatar id={a.id} size={20} />
            <span>
              {a.name}
              <span className="mono b-faint block">{a.node.replace("aidash://", "")}</span>
            </span>
          </span>
        ))}
        {policies.map((p) => (
          <div key={p.id} className="contents" role="row">
            <span className="b-mx-row">
              <span className="b-mx-name">{p.name}</span>
              <span className="mono b-faint b-mx-meta">
                <span>{p.id}</span>
                <span>r{p.revision}</span>
                <span>{p.scope}</span>
              </span>
            </span>
            {agents.map((a) => {
              const why = applies(p.scope, a.id);
              const n = audit.filter((x) => x.policy === p.id && x.actor === a.id).length;
              const active = cell[0] === p.id && cell[1] === a.id;
              return (
                <button
                  key={a.id}
                  type="button"
                  role="gridcell"
                  className="b-cell"
                  data-effect={why ? p.effect : undefined}
                  data-hit={n > 0}
                  aria-pressed={active}
                  aria-label={`${p.name} / ${a.name}: ${why ? effectLabel[p.effect] : "適用外"}`}
                  onClick={() => setCell([p.id, a.id])}
                >
                  {why ? (
                    <>
                      <span>{effectLabel[p.effect]}</span>
                      {n > 0 && <span className="mono b-cell-n">{n}</span>}
                    </>
                  ) : (
                    <span className="b-faint">·</span>
                  )}
                </button>
              );
            })}
          </div>
        ))}
      </div>
      <div className="b-mx-detail">
        <div>
          <span className="b-faint">選択中</span>
          <p>
            {policy.name} <span className="b-faint">×</span> {agentName(cell[1])}
          </p>
        </div>
        <div>
          <span className="b-faint">効果</span>
          <p>{reason ? effectLabel[policy.effect] : "適用外"}</p>
        </div>
        <div>
          <span className="b-faint">適用理由</span>
          <p>{reason ?? "スコープ外"}</p>
        </div>
        <div className="min-w-0 flex-[2]">
          <span className="b-faint">監査ヒット</span>
          {hits.length === 0 ? (
            <p className="b-faint">なし</p>
          ) : (
            hits.map((h) => (
              <p key={h.at}>
                <span className="mono b-faint">{h.at.slice(-8)}</span> <span className="mono">{h.action}</span> → {h.target}{" "}
                <span className="b-tag" data-tone={tones[h.decision]}>
                  {decisionLabel[h.decision]}
                </span>
              </p>
            ))
          )}
        </div>
      </div>
    </section>
  );
}

function Incidents() {
  return (
    <section className="b-sec">
      <header className="b-sec-head">
        <h2>インシデント</h2>
        <span className="b-faint">
          未解決 <span className="mono">{incidents.filter((i) => i.status === "open").length}</span>
        </span>
      </header>
      <ol className="b-incidents">
        {incidents.map((i) => (
          <li key={i.id} data-status={i.status}>
            <span className="b-sev mono" data-sev={i.severity}>
              {i.severity}
            </span>
            <div className="min-w-0">
              <p className="b-inc-title">{i.title}</p>
              <p className="b-faint">
                <span className="mono">{i.id}</span> · <span className="mono">{i.openedAt}</span> ·{" "}
                {i.status === "open" ? "対応中" : "解決済み"}
              </p>
            </div>
          </li>
        ))}
      </ol>
    </section>
  );
}

function AuditLog({ decision }: { decision: Decision }) {
  const [filter, setFilter] = useState<"all" | "allow" | "deny" | "pending">("all");
  const rows = audit
    .map((a) =>
      a.decision === "pending" && decision !== "pending"
        ? { ...a, decision: decision === "approved" ? "allow" : "deny" }
        : a,
    )
    .filter((a) => filter === "all" || a.decision === filter);
  return (
    <section className="b-sec">
      <header className="b-sec-head">
        <h2>監査ログ</h2>
        <div className="b-seg">
          {(["all", "allow", "deny", "pending"] as const).map((k) => (
            <button key={k} type="button" aria-pressed={filter === k} onClick={() => setFilter(k)}>
              {k === "all" ? "すべて" : decisionLabel[k]}
            </button>
          ))}
        </div>
        <label className="b-search ml-auto" style={{ width: 240 }}>
          <Search size={14} className="b-faint" />
          <input placeholder="操作・対象・ポリシーで検索" aria-label="監査ログを検索" />
        </label>
      </header>
      <div className="b-audit" role="table">
        <div className="b-audit-row b-audit-head" role="row">
          <span />
          <span>日時</span>
          <span>主体</span>
          <span>操作</span>
          <span>対象</span>
          <span>ポリシー</span>
          <span>判定</span>
        </div>
        {rows.map((a) => {
          const isAgent = agents.some((x) => x.id === a.actor);
          return (
            <div key={a.at} className="b-audit-row" role="row" data-decision={a.decision}>
              <span className="b-bar" />
              <span className="mono b-muted">{a.at}</span>
              <span className="flex items-center gap-2">
                {isAgent ? <Avatar id={a.actor} size={18} /> : <span className="b-ava b-ava-human" style={{ "--s": "18px" } as React.CSSProperties}>{a.actor.slice(0, 1)}</span>}
                {agentName(a.actor)}
              </span>
              <span className="mono">{a.action}</span>
              <span className="b-muted">{a.target}</span>
              <span className="mono b-faint">{a.policy}</span>
              <span className="b-decision-cell" data-tone={tones[a.decision]}>
                <span className="b-dot" />
                {decisionLabel[a.decision]}
              </span>
            </div>
          );
        })}
      </div>
    </section>
  );
}

export function GovernScreen({ decision }: { decision: Decision }) {
  const [range, setRange] = useState("24h");
  return (
    <div className="b-govern">
      <header className="b-govern-head">
        <div>
          <h1 className="b-title">統制</h1>
          <p className="b-muted">
            テナント <span className="mono">aidash-core</span> のポリシー適用状況と監査記録
          </p>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <div className="b-seg">
            {[
              ["24h", "24時間"],
              ["7d", "7日"],
              ["30d", "30日"],
            ].map(([k, l]) => (
              <button key={k} type="button" aria-pressed={range === k} onClick={() => setRange(k)}>
                {l}
              </button>
            ))}
          </div>
          <button type="button" className="b-btn">
            <Download size={14} />
            監査ログを書き出す
          </button>
        </div>
      </header>
      <Metrics decision={decision} />
      <div className="b-govern-grid">
        <Matrix />
        <Incidents />
      </div>
      <AuditLog decision={decision} />
    </div>
  );
}
