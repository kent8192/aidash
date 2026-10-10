// PROTOTYPE ONLY. Governance desk: policies + metrics / incidents + audit log / inspector.
import { useState } from "react";
import { ArrowUpRight, Download, Plus, Search } from "lucide-react";
import { agents, audit, incidents, policies, trustMetrics } from "../data";
import type { Screen } from "../types";
import { Glyph, Pane, PaneHeader, SectionTitle, nodeLabel, statusTone, toneColor } from "./ui";

type Decision = "all" | "allow" | "deny" | "pending";
type AuditRow = (typeof audit)[number];

const effectLabel: Record<string, string> = {
  "require-approval": "承認が必要",
  deny: "拒否",
  allow: "許可",
  limit: "上限",
};

export function GovernScreen({ onScreen }: { onScreen: (s: Screen) => void }) {
  const [policy, setPolicy] = useState<string | null>(null);
  const [decision, setDecision] = useState<Decision>("all");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<AuditRow>(audit[0]);

  const rows = audit.filter(
    (a) =>
      (policy === null || a.policy === policy) &&
      (decision === "all" || a.decision === decision) &&
      `${a.actor} ${a.action} ${a.target}`.toLowerCase().includes(query.trim().toLowerCase()),
  );

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[268px_minmax(0,1fr)_300px] min-[1400px]:grid-cols-[296px_minmax(0,1fr)_352px]">
      <Pane className="border-r border-(--a-line) bg-(--a-bg)">
        <PaneHeader>
          <h2 className="m-0 text-[13px] font-semibold">統制</h2>
          <span className="mono text-[12px] text-(--a-faint)">aidash-core</span>
        </PaneHeader>
        <div className="a-scroll flex-1 pb-16">
          <dl className="m-0 grid grid-cols-2 gap-x-4 gap-y-3 border-b border-(--a-line) px-4 py-3.5">
            {trustMetrics.map((m) => (
              <div key={m.label} className="min-w-0">
                <dt className="text-[12px] text-(--a-muted)">{m.label}</dt>
                <dd className="m-0 flex items-baseline gap-2">
                  <span className="mono text-[20px] leading-[1.3] font-medium">{m.value}</span>
                </dd>
                <dd className="mono m-0 text-[11px] leading-[1.5] text-(--a-faint)">{m.delta}</dd>
              </div>
            ))}
          </dl>
          <div className="pt-3">
            <div className="flex items-center justify-between px-4 pb-1">
              <span className="a-eyebrow">ポリシー</span>
              <button type="button" className="a-btn a-btn-ghost -mr-2 h-6 px-1.5">
                <Plus size={13} />
                追加
              </button>
            </div>
            <ul className="m-0 list-none p-0" role="listbox" aria-label="ポリシー">
              <li>
                <button
                  type="button"
                  role="option"
                  aria-selected={policy === null}
                  onClick={() => setPolicy(null)}
                  className="a-row flex w-full cursor-pointer items-center border-0 bg-transparent px-4 py-2 text-left"
                >
                  <span className="flex-1">すべての記録</span>
                  <span className="mono text-[12px] text-(--a-faint)">{audit.length}</span>
                </button>
              </li>
              {policies.map((p) => {
                const hits = audit.filter((a) => a.policy === p.id).length;
                return (
                  <li key={p.id}>
                    <button
                      type="button"
                      role="option"
                      aria-selected={policy === p.id}
                      onClick={() => setPolicy(p.id)}
                      className="a-row block w-full cursor-pointer border-0 bg-transparent px-4 py-2 text-left"
                    >
                      <span className="flex items-baseline gap-2">
                        <span className="min-w-0 flex-1 leading-[1.5]">{p.name}</span>
                        <span className="mono text-[12px] text-(--a-faint)">{hits}</span>
                      </span>
                      <span className="mono mt-0.5 flex items-center gap-2 text-[11.5px]">
                        <span className="shrink-0 whitespace-nowrap" style={{ color: p.effect === "limit" ? "var(--a-muted)" : toneColor[statusTone(p.effect)] }}>
                          {p.effect}
                        </span>
                        <span className="truncate text-(--a-faint)">{p.scope}</span>
                        <span className="ml-auto shrink-0 text-(--a-faint)">r{p.revision}</span>
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        </div>
      </Pane>

      <Pane className="bg-(--a-surface)">
        <PaneHeader>
          <h2 className="m-0 text-[13px] font-semibold">インシデント</h2>
          <span className="mono text-[12px] text-(--a-faint)">
            {incidents.filter((i) => i.status === "open").length} 件未解決
          </span>
        </PaneHeader>
        <ul className="m-0 shrink-0 list-none border-b border-(--a-line) p-0 py-1">
          {incidents.map((i) => {
            const resolved = i.status === "resolved";
            return (
              <li key={i.id}>
                <button
                  type="button"
                  className="a-row grid w-full cursor-pointer grid-cols-[14px_64px_minmax(0,1fr)_64px_88px_96px] items-center gap-x-2 border-0 bg-transparent px-4 py-1.5 text-left"
                >
                  <Glyph tone={resolved ? "idle" : statusTone(i.severity)} hollow={resolved} />
                  <span className="mono text-[12px] text-(--a-muted)">{i.id}</span>
                  <span className={`truncate ${resolved ? "text-(--a-muted)" : ""}`}>{i.title}</span>
                  <span className="mono text-[12px]" style={{ color: resolved ? "var(--a-faint)" : toneColor[statusTone(i.severity)] }}>
                    {i.severity}
                  </span>
                  <span className="text-[12px] text-(--a-muted)">{resolved ? "解決済み" : "対応中"}</span>
                  <span className="mono text-right text-[12px] text-(--a-faint)">{i.openedAt}</span>
                </button>
              </li>
            );
          })}
        </ul>

        <div className="flex h-11 shrink-0 items-center gap-3 px-4">
          <h2 className="m-0 shrink-0 text-[13px] font-semibold whitespace-nowrap">監査ログ</h2>
          <span className="mono text-[12px] text-(--a-faint)">
            {rows.length}/{audit.length}
          </span>
          {policy ? (
            <button type="button" className="a-chip mono" aria-pressed onClick={() => setPolicy(null)}>
              policy:{policy} ×
            </button>
          ) : null}
          <span className="flex-1" />
          <label className="flex h-7 w-[180px] items-center gap-2 rounded-(--a-r-control) border border-(--a-line) bg-(--a-surface) px-2 focus-within:border-(--a-accent) min-[1400px]:w-[240px]">
            <Search size={13} className="shrink-0 text-(--a-faint)" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="主体・操作・対象で絞り込み"
              className="h-full min-w-0 flex-1 border-0 bg-transparent text-[12px] text-(--a-text) outline-none"
            />
          </label>
          <div className="a-seg" role="group" aria-label="判定">
            {(["all", "allow", "deny", "pending"] as Decision[]).map((d) => (
              <button key={d} type="button" aria-pressed={decision === d} onClick={() => setDecision(d)} className={d === "all" ? "" : "mono"}>
                {d === "all" ? "すべて" : d}
              </button>
            ))}
          </div>
          <button type="button" className="a-icon-btn" aria-label="書き出し">
            <Download size={14} />
          </button>
        </div>

        <div className="a-scroll min-h-0 flex-1 pb-16">
          <table className="a-table">
            <colgroup>
              <col style={{ width: 128 }} />
              <col style={{ width: 98 }} />
              <col style={{ width: 172 }} />
              <col />
              <col style={{ width: 86 }} />
              <col style={{ width: 110 }} />
            </colgroup>
            <thead>
              <tr>
                <th className="pl-4">時刻</th>
                <th>主体</th>
                <th>操作</th>
                <th>対象</th>
                <th>判定</th>
                <th className="pr-4">ポリシー</th>
              </tr>
            </thead>
            <tbody className="text-[12px]">
              {rows.map((a) => {
                const tone = statusTone(a.decision);
                return (
                  <tr
                    key={a.at + a.action}
                    aria-selected={a === selected}
                    tabIndex={0}
                    onClick={() => setSelected(a)}
                    onKeyDown={(e) => e.key === "Enter" && setSelected(a)}
                    className="cursor-pointer"
                  >
                    <td className="mono pl-4 text-(--a-muted)">{a.at}</td>
                    <td className={agents.some((g) => g.id === a.actor) ? "mono" : ""}>{a.actor}</td>
                    <td className="mono">{a.action}</td>
                    <td className="mono text-(--a-muted)">{a.target}</td>
                    <td>
                      <span className="mono inline-flex items-center gap-1.5" style={{ color: toneColor[tone] }}>
                        <Glyph tone={tone} size={6} hollow={a.decision === "pending"} />
                        {a.decision}
                      </span>
                    </td>
                    <td className="mono pr-4 text-(--a-faint)">{a.policy}</td>
                  </tr>
                );
              })}
              {rows.length === 0 ? (
                <tr>
                  <td colSpan={6} className="h-24 text-center text-(--a-muted)">
                    条件に一致する記録はありません
                  </td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </div>
      </Pane>

      <AuditInspector row={selected} onScreen={onScreen} />
    </div>
  );
}

function AuditInspector({ row, onScreen }: { row: AuditRow; onScreen: (s: Screen) => void }) {
  const governing = policies.find((p) => p.id === row.policy);
  const agent = agents.find((a) => a.id === row.actor);
  const tone = statusTone(row.decision);
  const related = audit.filter((a) => a.policy === row.policy && a !== row);
  const fields: [string, string, boolean][] = [
    ["時刻", `2026/${row.at}`, true],
    ["主体", row.actor, Boolean(agent)],
    ["ノード", agent ? agent.node : "aidash://home", true],
    ["モデル", agent ? agent.model : "人間の操作", Boolean(agent)],
    ["対象", row.target, true],
  ];
  return (
    <Pane className="border-l border-(--a-line) bg-(--a-bg)">
      <PaneHeader>
        <h2 className="m-0 text-[13px] font-semibold">記録の詳細</h2>
        <span className="flex-1" />
        <span className="mono text-[12px] text-(--a-faint)">{nodeLabel(agent?.node ?? "aidash://home")}</span>
      </PaneHeader>
      <div className="a-scroll flex-1 pb-16">
        <section className="border-b border-(--a-line) px-4 py-3.5">
          <div className="mono text-[15px] font-medium">{row.action}</div>
          <div className="mt-1 flex items-center gap-2 text-[12px]" style={{ color: toneColor[tone] }}>
            <Glyph tone={tone} size={7} hollow={row.decision === "pending"} />
            <span className="mono">{row.decision}</span>
            <span className="text-(--a-muted)">
              {row.decision === "allow" ? "実行を許可" : row.decision === "deny" ? "実行を拒否" : "人間の承認を待機中"}
            </span>
          </div>
          {row.decision === "pending" ? (
            <button type="button" className="a-btn a-btn-primary mt-3 h-8" onClick={() => onScreen("request")}>
              依頼で判断する
              <ArrowUpRight size={14} />
            </button>
          ) : null}
        </section>

        <section className="border-b border-(--a-line) px-4 py-3.5">
          <dl className="m-0 grid grid-cols-[64px_minmax(0,1fr)] gap-x-3 gap-y-1.5 leading-[1.5]">
            {fields.map(([k, v, mono]) => (
              <div key={k} className="contents">
                <dt className="text-[12px] text-(--a-muted)">{k}</dt>
                <dd className={`m-0 min-w-0 break-words ${mono ? "mono text-[12px]" : ""}`}>{v}</dd>
              </div>
            ))}
          </dl>
        </section>

        <section className="border-b border-(--a-line) px-4 py-3.5">
          <SectionTitle aside={row.policy}>適用されたポリシー</SectionTitle>
          {governing ? (
            <>
              <p className="m-0 font-medium leading-[1.5]">{governing.name}</p>
              <dl className="mono m-0 mt-2 grid grid-cols-[64px_minmax(0,1fr)] gap-x-3 gap-y-1 text-[12px]">
                <dt className="sans text-(--a-muted)">効果</dt>
                <dd className="m-0" style={{ color: governing.effect === "limit" ? undefined : toneColor[statusTone(governing.effect)] }}>
                  {governing.effect}
                  <span className="sans ml-2 text-(--a-muted)">{effectLabel[governing.effect]}</span>
                </dd>
                <dt className="sans text-(--a-muted)">範囲</dt>
                <dd className="m-0">{governing.scope}</dd>
                <dt className="sans text-(--a-muted)">改訂</dt>
                <dd className="m-0">
                  r{governing.revision} · {governing.updatedAt}
                </dd>
              </dl>
              <div className="mt-3 flex gap-1.5">
                <button type="button" className="a-btn h-7">
                  ポリシーを開く
                </button>
                <button type="button" className="a-btn a-btn-ghost h-7">
                  改訂履歴
                </button>
              </div>
            </>
          ) : (
            <p className="m-0 leading-[1.6] text-(--a-muted)">
              <span className="mono text-(--a-text)">{row.policy}</span>
              {row.policy === "admin" ? " 管理者権限による操作です。" : " 明示的なポリシーがなく、既定の規則で判定されました。"}
            </p>
          )}
        </section>

        <section className="px-4 py-3.5">
          <SectionTitle aside={String(related.length)}>同じポリシーの記録</SectionTitle>
          {related.length === 0 ? (
            <p className="m-0 text-[12px] text-(--a-muted)">ほかの記録はありません</p>
          ) : (
            <ul className="m-0 list-none p-0">
              {related.slice(0, 5).map((r) => (
                <li key={r.at + r.action} className="mono grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 py-1 text-[12px]">
                  <span className="truncate">{r.action}</span>
                  <span style={{ color: toneColor[statusTone(r.decision)] }}>{r.decision}</span>
                  <span className="col-span-2 text-[11px] text-(--a-faint)">
                    {r.at} · {r.actor}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </Pane>
  );
}
