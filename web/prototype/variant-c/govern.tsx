// PROTOTYPE ONLY. Variant C govern screen: a long-form governance record with a sticky table of contents.
import { useEffect, useState } from "react";
import { Download, Plus } from "lucide-react";
import { agentName, audit, incidents, policies, trustMetrics, viewer } from "../data";
import type { Screen } from "../types";
import { NOW, TopStrip } from "./parts";

const effectWord: Record<string, { word: string; tone: string }> = {
  allow: { word: "許可", tone: "muted" },
  deny: { word: "拒否", tone: "tone-error" },
  "require-approval": { word: "承認が必要", tone: "tone-warn" },
  limit: { word: "上限", tone: "text-[var(--c-ink)]" },
};

const decisionWord: Record<string, { word: string; tone: string }> = {
  allow: { word: "許可", tone: "muted" },
  deny: { word: "拒否", tone: "tone-error" },
  pending: { word: "承認待ち", tone: "tone-warn" },
};

const sections = [
  { id: "c-gov-overview", label: "概要", count: null as number | null },
  { id: "c-gov-policies", label: "ポリシー", count: policies.length },
  { id: "c-gov-audit", label: "監査ログ", count: audit.length },
  { id: "c-gov-incidents", label: "インシデント", count: incidents.length },
];

type AuditFilter = "all" | "deny" | "pending";

export function GovernScreen({ onScreen }: { onScreen: (s: Screen) => void }) {
  const [active, setActive] = useState(sections[0].id);
  const [filter, setFilter] = useState<AuditFilter>("all");

  // Scroll spy: the topmost section crossing the upper third of the viewport is current.
  useEffect(() => {
    const root = document.querySelector(".proto-c .c-main");
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setActive(visible[0].target.id);
      },
      { root, rootMargin: "-64px 0px -60% 0px" },
    );
    for (const s of sections) {
      const el = document.getElementById(s.id);
      if (el) observer.observe(el);
    }
    return () => observer.disconnect();
  }, []);

  const openIncidents = incidents.filter((i) => i.status === "open").length;
  const rows = audit.filter((row) => filter === "all" || row.decision === filter);

  return (
    <>
      <TopStrip crumbs={[{ label: "統制" }]} note={`${NOW} に更新`}>
        <button type="button" className="c-btn c-btn--sm">
          <Download size={14} />
          監査ログを書き出す
        </button>
        <button type="button" className="c-btn c-btn--primary c-btn--sm">
          <Plus size={14} />
          ポリシーを追加
        </button>
      </TopStrip>

      <div className="c-page c-page--toc">
        <article className="c-doc">
          <section id="c-gov-overview" className="scroll-mt-[72px]">
            <div className="c-eyebrow">
              <span>{viewer.tenant}</span>
              <span aria-hidden>·</span>
              <span>Trust</span>
            </div>
            <h1 className="c-title">統制</h1>
            <p className="c-lede">
              {viewer.tenant} で有効なポリシーと、その判定の記録です。現在、承認待ちが 1 件、未解決のインシデントが {openIncidents}{" "}
              件あります。
            </p>
            <h2 className="c-h2">概要</h2>
            <dl className="c-metrics">
              {trustMetrics.map((metric) => (
                <div key={metric.label}>
                  <dt>{metric.label}</dt>
                  <dd>
                    {metric.value}
                    <small>{metric.delta}</small>
                  </dd>
                </div>
              ))}
            </dl>
            <p className="c-section-note" style={{ marginTop: 14 }}>
              承認待ちの 1 件は v0.1-release のパッケージ公開です。
              <button type="button" className="c-link" onClick={() => onScreen("request")}>
                依頼で判断する
              </button>
            </p>
          </section>

          <section id="c-gov-policies" className="scroll-mt-[72px]">
            <h2 className="c-h2">
              ポリシー
              <small className="num">{policies.length} 件を表示 · 有効 23 件</small>
            </h2>
            <p className="c-section-note">
              エージェントの操作は、該当するポリシーの効果で判定されます。拒否は承認や許可より優先されます。
            </p>
            <ul className="c-policies">
              {policies.map((policy) => {
                const effect = effectWord[policy.effect];
                return (
                  <li key={policy.id} className="c-policy">
                    <div className="min-w-0">
                      <div className="c-policy__name">{policy.name}</div>
                      <div className="c-policy__sub">
                        <span className="mono">{policy.id}</span>
                        <span>
                          適用範囲 <span className="mono text-[var(--c-ink-2)]">{policy.scope}</span>
                        </span>
                      </div>
                    </div>
                    <div className="c-policy__effect">
                      <div className={effect.tone}>{effect.word}</div>
                      <div className="text-[12px] font-normal text-[var(--c-muted)] num">
                        r{policy.revision} · {policy.updatedAt} 更新
                      </div>
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>

          <section id="c-gov-audit" className="scroll-mt-[72px]">
            <h2 className="c-h2">
              監査ログ
              <small>直近の判定</small>
            </h2>
            <div className="c-tabs" role="group" aria-label="判定で絞り込む">
              {(
                [
                  ["all", "すべて"],
                  ["deny", "拒否"],
                  ["pending", "承認待ち"],
                ] as [AuditFilter, string][]
              ).map(([key, label]) => (
                <button key={key} type="button" aria-pressed={filter === key} onClick={() => setFilter(key)}>
                  {label}
                  <span className="num">{audit.filter((row) => key === "all" || row.decision === key).length}</span>
                </button>
              ))}
            </div>
            <table className="c-table c-table--audit">
              <thead>
                <tr>
                  <th>時刻</th>
                  <th>主体</th>
                  <th>操作</th>
                  <th>対象</th>
                  <th>判定</th>
                  <th>ポリシー</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const decision = decisionWord[row.decision];
                  return (
                    <tr key={row.at}>
                      <td className="mono whitespace-nowrap text-[var(--c-muted)]">{row.at}</td>
                      <td className="whitespace-nowrap">{agentName(row.actor)}</td>
                      <td className="mono">{row.action}</td>
                      <td className="text-[var(--c-ink-2)] [overflow-wrap:anywhere]">{row.target}</td>
                      <td className={`whitespace-nowrap font-medium ${decision.tone}`}>{decision.word}</td>
                      <td className="mono text-[var(--c-muted)]">{row.policy}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </section>

          <section id="c-gov-incidents" className="scroll-mt-[72px]">
            <h2 className="c-h2">
              インシデント
              <small className="num">未解決 {openIncidents} 件</small>
            </h2>
            <ol className="c-incidents">
              {incidents.map((incident) => (
                <li
                  key={incident.id}
                  className={`c-incident${incident.status === "resolved" ? " c-incident--resolved" : ""}`}
                >
                  <div className="min-w-0">
                    <div className="c-incident__title">{incident.title}</div>
                    <div className="c-incident__meta">
                      <span className="mono">{incident.id}</span>
                      <span>
                        重大度{" "}
                        <span className={incident.severity === "high" ? "tone-error font-medium" : "tone-warn font-medium"}>
                          {incident.severity}
                        </span>
                      </span>
                      <span className="num">{incident.openedAt} に発生</span>
                    </div>
                  </div>
                  <div className="text-right text-[13.5px] leading-[1.75]">
                    <div className={incident.status === "open" ? "font-medium" : "muted"}>
                      {incident.status === "open" ? "未解決" : "解決済み"}
                    </div>
                    <button type="button" className="c-link text-[12.5px]">
                      記録を開く
                    </button>
                  </div>
                </li>
              ))}
            </ol>
          </section>
        </article>

        <aside className="c-aside" aria-label="目次">
          <div className="c-aside__block">
            <div className="c-aside__label">このページ</div>
            <nav className="c-toc">
              {sections.map((s) => (
                <a key={s.id} href={`#${s.id}`} aria-current={active === s.id ? "true" : undefined}>
                  <span>{s.label}</span>
                  {s.count !== null && <span className="num text-[12px] font-normal text-[var(--c-muted)]">{s.count}</span>}
                </a>
              ))}
            </nav>
          </div>
          <div className="c-aside__block">
            <div className="c-aside__label">権限</div>
            <p className="mono text-[12px] leading-[1.7] text-[var(--c-ink-2)]">{viewer.authority}</p>
            <p className="mt-1 text-[12.5px] text-[var(--c-muted)]">ポリシーの閲覧と承認ができます。</p>
          </div>
        </aside>
      </div>
    </>
  );
}
