// PROTOTYPE ONLY. Request desk: inbox / conversation timeline / inspector.
import { useState } from "react";
import {
  AtSign,
  CornerDownLeft,
  FileCode2,
  FileText,
  Filter,
  MessageSquare,
  MoreHorizontal,
  Paperclip,
  Plus,
  Share2,
  Undo2,
} from "lucide-react";
import {
  agentName,
  agents,
  artifacts,
  events,
  humanRequest,
  messages,
  runs,
  statusLabel,
  tasks,
  viewer,
  workspaces,
} from "../data";
import type { Message } from "../data";
import { Glyph, Kbd, Pane, PaneHeader, SectionTitle, duration, nodeLabel, relativeTime, statusTone, toMinutes, toneColor } from "./ui";

export type Decision = "pending" | "approved" | "rejected";

const inboxGroups = [
  { label: "対応が必要", items: workspaces.filter((w) => w.pending > 0) },
  { label: "進行中", items: workspaces.filter((w) => w.pending === 0 && w.status === "RUNNING") },
  { label: "完了・失敗", items: workspaces.filter((w) => w.status === "COMPLETED" || w.status === "FAILED") },
];

export function RequestScreen({
  decision,
  onDecision,
  selected,
  onSelect,
}: {
  decision: Decision;
  onDecision: (d: Decision) => void;
  selected: string;
  onSelect: (id: string) => void;
}) {
  const workspace = workspaces.find((w) => w.id === selected) ?? workspaces[0];
  const primary = workspace.id === workspaces[0].id;
  return (
    <div className="grid min-h-0 flex-1 grid-cols-[280px_minmax(0,1fr)_340px]">
      <Inbox selected={workspace.id} onSelect={onSelect} />
      <Conversation workspace={workspace} primary={primary} decision={decision} onDecision={onDecision} />
      <Inspector workspace={workspace} primary={primary} />
    </div>
  );
}

function Inbox({ selected, onSelect }: { selected: string; onSelect: (id: string) => void }) {
  return (
    <Pane className="border-r border-(--a-line) bg-(--a-bg)">
      <PaneHeader>
        <h2 className="m-0 text-[13px] font-semibold">受信箱</h2>
        <span className="mono text-[12px] text-(--a-faint)">{workspaces.length}</span>
        <span className="flex-1" />
        <button type="button" className="a-icon-btn" aria-label="絞り込み">
          <Filter size={14} />
        </button>
        <button type="button" className="a-btn h-7 px-2" aria-label="新しい依頼">
          <Plus size={14} />
          <Kbd>N</Kbd>
        </button>
      </PaneHeader>
      <div className="a-scroll flex-1 pb-4">
        {inboxGroups.map((g) => (
          <div key={g.label} className="pt-3">
            <div className="flex items-center justify-between px-4 pb-1">
              <span className="a-eyebrow">{g.label}</span>
              <span className="mono text-[11px] text-(--a-faint)">{g.items.length}</span>
            </div>
            <ul className="m-0 list-none p-0" role="listbox" aria-label={g.label}>
              {g.items.map((w) => (
                <li key={w.id}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={w.id === selected}
                    onClick={() => onSelect(w.id)}
                    className="a-row block w-full cursor-pointer border-0 bg-transparent px-4 py-2 text-left"
                  >
                    <span className="flex items-center gap-2">
                      <Glyph tone={w.pending > 0 ? "warn" : statusTone(w.status)} />
                      <span className="mono min-w-0 flex-1 truncate text-[13px] font-medium text-(--a-text)">
                        {w.title}
                      </span>
                      <span className="shrink-0 text-[11.5px] text-(--a-faint)">{relativeTime(w.updatedAt)}</span>
                    </span>
                    <span className="mt-0.5 block truncate pl-[15px] text-[12px] leading-[1.6] text-(--a-muted)">
                      {w.goal}
                    </span>
                    <span className="mt-1 flex items-center gap-2 pl-[15px]">
                      <span className="relative h-[3px] w-14 overflow-hidden bg-(--a-line)">
                        <span
                          className="absolute inset-y-0 left-0"
                          style={{
                            width: `${(w.tasksDone / w.tasksTotal) * 100}%`,
                            background: w.status === "FAILED" ? toneColor.fail : w.status === "COMPLETED" ? toneColor.done : toneColor.accent,
                          }}
                        />
                      </span>
                      <span className="mono text-[11.5px] text-(--a-muted)">
                        {w.tasksDone}/{w.tasksTotal}
                      </span>
                      <span className="text-[11.5px]" style={{ color: w.pending > 0 ? toneColor.warn : w.status === "FAILED" ? toneColor.fail : "var(--a-faint)" }}>
                        {w.pending > 0 ? `承認待ち ${w.pending}` : statusLabel[w.status]}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </Pane>
  );
}

type TimelineItem =
  | { type: "message"; at: string; sort: number; message: Message }
  | { type: "event"; at: string; sort: number; actor: string; kind: string; text: string; warn: boolean };

const timeline: TimelineItem[] = [
  ...messages.map((m) => ({ type: "message" as const, at: m.at, sort: toMinutes(m.at), message: m })),
  ...events
    .filter((e) => e.kind !== "request")
    .map((e) => ({
      type: "event" as const,
      at: e.at.slice(0, 5),
      sort: toMinutes(e.at),
      actor: e.actor,
      kind: e.kind,
      text: e.text,
      warn: e.severity === "warn",
    })),
].sort((a, b) => a.sort - b.sort);

function Conversation({
  workspace,
  primary,
  decision,
  onDecision,
}: {
  workspace: (typeof workspaces)[number];
  primary: boolean;
  decision: Decision;
  onDecision: (d: Decision) => void;
}) {
  const [showEvents, setShowEvents] = useState(true);
  const items = showEvents ? timeline : timeline.filter((i) => i.type === "message");
  return (
    <Pane className="bg-(--a-surface)">
      <PaneHeader>
        <span className="text-[12px] text-(--a-muted)">依頼</span>
        <span className="text-(--a-line)">/</span>
        <h1 className="mono m-0 truncate text-[13px] font-semibold">{workspace.title}</h1>
        <span className="flex items-center gap-1.5 text-[12px]" style={{ color: toneColor[statusTone(workspace.status)] }}>
          <Glyph tone={statusTone(workspace.status)} size={6} />
          {statusLabel[workspace.status]}
        </span>
        <span className="mono text-[12px] text-(--a-muted)">
          {workspace.tasksDone}/{workspace.tasksTotal} タスク
        </span>
        <span className="flex-1" />
        <div className="a-seg" role="group" aria-label="表示">
          <button type="button" aria-pressed={showEvents} onClick={() => setShowEvents(true)}>
            すべて
          </button>
          <button type="button" aria-pressed={!showEvents} onClick={() => setShowEvents(false)}>
            会話のみ
          </button>
        </div>
        <button type="button" className="a-icon-btn" aria-label="共有">
          <Share2 size={14} />
        </button>
        <button type="button" className="a-icon-btn" aria-label="その他">
          <MoreHorizontal size={15} />
        </button>
      </PaneHeader>

      {primary ? <DecisionStrip decision={decision} onDecision={onDecision} /> : null}

      <div className="a-scroll min-h-0 flex-1">
        {primary ? (
          <ol className="m-0 list-none px-0 pt-3 pb-6">
            {items.map((item, index) =>
              item.type === "message" ? (
                <MessageRow key={item.message.id} message={item.message} />
              ) : (
                <li
                  key={`e${index}`}
                  className="mono grid grid-cols-[64px_minmax(0,1fr)] items-baseline px-5 text-[12px] leading-[22px]"
                >
                  <span className="text-(--a-faint)">{item.at}</span>
                  <span className="truncate text-(--a-muted)">
                    <span className="text-(--a-text)">{item.actor}</span>
                    <span className="text-(--a-faint)"> → {item.kind} </span>
                    <span style={item.warn ? { color: toneColor.warn } : undefined}>{item.text}</span>
                  </span>
                </li>
              ),
            )}
            <li className="mt-3 grid grid-cols-[64px_minmax(0,1fr)] items-center px-5 text-[12px]">
              <span className="mono text-(--a-accent)">09:42</span>
              <span className="flex items-center gap-2 text-(--a-muted)">
                <span className="a-pulse text-(--a-accent)" />
                Verifier が思考中
                <span className="mono text-(--a-faint)">run-2 · step 9 · {runs[2].duration}</span>
              </span>
            </li>
          </ol>
        ) : (
          <div className="grid h-full place-items-center px-8 text-center text-(--a-muted)">
            <div>
              <p className="m-0 font-medium text-(--a-text)">{workspace.goal}</p>
              <p className="m-0 mt-1 text-[12px]">この依頼の会話はプロトタイプのデータに含まれていません。</p>
            </div>
          </div>
        )}
      </div>

      <Composer />
    </Pane>
  );
}

function MessageRow({ message }: { message: Message }) {
  const agent = agents.find((a) => a.id === message.sender);
  const isHuman = message.sender === "human";
  const isSystem = message.sender === "system";
  const artifact = artifacts.find((a) => a.name === message.attachment);
  return (
    <li className="grid grid-cols-[64px_minmax(0,1fr)] px-5 py-2.5">
      <span className="mono pt-px text-[12px] text-(--a-faint)">{message.at}</span>
      <div className="min-w-0">
        <div className="flex items-baseline gap-2 leading-[1.5]">
          <span className={`font-semibold ${isSystem ? "text-(--a-muted)" : ""}`}>{agentName(message.sender)}</span>
          {agent ? (
            <span className="mono text-[11.5px] text-(--a-faint)">
              {nodeLabel(agent.node)} · {agent.model}
            </span>
          ) : null}
          {isHuman ? <span className="text-[11.5px] text-(--a-faint)">依頼者</span> : null}
        </div>
        <p className={`m-0 mt-0.5 max-w-[68ch] leading-[1.6] ${isSystem ? "text-(--a-muted)" : ""}`}>{message.content}</p>
        {artifact ? (
          <button
            type="button"
            className="mt-2 flex w-full max-w-[440px] cursor-pointer items-center gap-2.5 rounded-(--a-r-control) border border-(--a-line) bg-(--a-bg) px-2.5 py-1.5 text-left transition-colors hover:border-[#d0d5dd] hover:bg-(--a-surface)"
          >
            {artifact.kind === "patch" ? (
              <FileCode2 size={15} className="shrink-0 text-(--a-muted)" />
            ) : (
              <FileText size={15} className="shrink-0 text-(--a-muted)" />
            )}
            <span className="mono flex-1 truncate text-[12px]">{artifact.name}</span>
            <span className="mono text-[11.5px] text-(--a-faint)">{artifact.size}</span>
          </button>
        ) : null}
        {message.thread ? (
          <button type="button" className="a-btn a-btn-ghost mt-1 -ml-2 h-6 px-2 text-(--a-accent) hover:text-(--a-accent)">
            <MessageSquare size={13} />
            返信 {message.thread}件
          </button>
        ) : null}
      </div>
    </li>
  );
}

function DecisionStrip({ decision, onDecision }: { decision: Decision; onDecision: (d: Decision) => void }) {
  if (decision !== "pending") {
    return (
      <div className="flex h-10 shrink-0 items-center gap-2.5 border-b border-(--a-line) bg-(--a-bg) px-5 text-[12px]">
        <Glyph tone={decision === "approved" ? "done" : "fail"} size={6} />
        <span className="font-medium">{decision === "approved" ? "承認しました" : "却下しました"}</span>
        <span className="text-(--a-muted)">{humanRequest.prompt.replace("？", "")}</span>
        <span className="mono text-(--a-faint)">09:42 · {viewer.displayName}</span>
        <span className="flex-1" />
        <button type="button" className="a-btn a-btn-ghost h-6 px-2" onClick={() => onDecision("pending")}>
          <Undo2 size={13} />
          取り消す
          <Kbd>U</Kbd>
        </button>
      </div>
    );
  }
  return (
    <div className="shrink-0 border-b border-(--a-line) bg-(--a-text) px-5 py-3 text-white">
      <div className="flex items-center gap-2 text-[12px] text-[#aab3c2]">
        <Glyph tone="warn" size={6} />
        <span className="font-medium text-[#f0c27a]">{statusLabel[humanRequest.kind]}</span>
        <span className="mono">
          {humanRequest.agent} · {humanRequest.taskId} · {humanRequest.createdAt} から待機
        </span>
      </div>
      <div className="mt-1.5 flex flex-wrap items-center gap-x-6 gap-y-2.5">
        <div className="min-w-[260px] flex-1">
          <p className="m-0 text-[14px] leading-[1.5] font-semibold">{humanRequest.prompt}</p>
          <p className="m-0 mt-0.5 text-[12px] leading-[1.6] text-[#c3cad5]">{humanRequest.detail}</p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <button type="button" className="a-btn a-btn-primary h-8 px-3" onClick={() => onDecision("approved")}>
            承認
            <Kbd>A</Kbd>
          </button>
          <button
            type="button"
            className="a-btn h-8 border-[#3a4352] bg-transparent px-3 text-white hover:border-[#566074] hover:bg-[#1d2531] active:bg-[#252e3b]"
            onClick={() => onDecision("rejected")}
          >
            却下
            <kbd className="a-kbd border-[#3a4352] bg-transparent text-[#aab3c2]">R</kbd>
          </button>
          <button
            type="button"
            className="a-btn h-8 border-transparent bg-transparent px-3 text-[#c3cad5] hover:bg-[#1d2531] hover:text-white"
          >
            返信
          </button>
        </div>
      </div>
    </div>
  );
}

function Composer() {
  return (
    <div className="shrink-0 border-t border-(--a-line) bg-(--a-surface) px-4 pt-3 pb-2">
      <div className="rounded-(--a-r-control) border border-(--a-line) bg-(--a-surface) transition-colors focus-within:border-(--a-accent) focus-within:shadow-[0_0_0_3px_var(--a-accent-soft)]">
        <div className="flex items-center gap-1.5 px-3 pt-2 text-[12px] text-(--a-muted)">
          <span>宛先</span>
          {["publisher", "verifier"].map((id) => (
            <span key={id} className="mono rounded-[4px] bg-(--a-sunken) px-1.5 text-[11.5px] leading-[20px] text-(--a-text)">
              @{id}
            </span>
          ))}
          <span className="text-(--a-faint)">現在稼働中のエージェント</span>
        </div>
        <textarea
          rows={2}
          placeholder="指示や補足を入力。@ で宛先を変更"
          className="block w-full resize-none border-0 bg-transparent px-3 pt-1 text-[13px] leading-[1.6] text-(--a-text) outline-none"
        />
        <div className="flex items-center gap-1 px-1.5 pb-1.5">
          <button type="button" className="a-btn a-btn-ghost h-7 px-2">
            <Paperclip size={14} />
            添付
          </button>
          <button type="button" className="a-btn a-btn-ghost h-7 px-2">
            <AtSign size={14} />
            エージェント
          </button>
          <span className="flex-1" />
          <button type="button" className="a-btn a-btn-primary h-7 px-2.5">
            送信
            <span className="flex items-center gap-0.5">
              <Kbd>⌘</Kbd>
              <Kbd>
                <CornerDownLeft size={10} />
              </Kbd>
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}

function Inspector({ workspace, primary }: { workspace: (typeof workspaces)[number]; primary: boolean }) {
  return (
    <Pane className="border-l border-(--a-line) bg-(--a-bg)">
      <PaneHeader>
        <h2 className="m-0 text-[13px] font-semibold">詳細</h2>
        <span className="mono text-[12px] text-(--a-faint)">{workspace.id}</span>
      </PaneHeader>
      <div className="a-scroll flex-1 pb-16">
        <section className="border-b border-(--a-line) px-4 py-3.5">
          <SectionTitle>目標</SectionTitle>
          <p className="m-0 leading-[1.6]">{workspace.goal}</p>
        </section>

        {primary ? (
          <>
            <section className="border-b border-(--a-line) px-4 py-3.5">
              <SectionTitle aside={`${workspace.tasksDone}/${workspace.tasksTotal}`}>タスク</SectionTitle>
              <ol className="m-0 list-none p-0">
                {tasks.map((t, i) => {
                  const tone = statusTone(t.status);
                  const last = i === tasks.length - 1;
                  return (
                    <li key={t.id} className="relative grid grid-cols-[16px_minmax(0,1fr)] gap-x-2 pb-2.5">
                      {!last ? (
                        <span aria-hidden className="absolute top-[18px] bottom-[-2px] left-[7.5px] w-px bg-[#d5dae2]" />
                      ) : null}
                      <span className="relative mt-[7px] flex justify-center">
                        <Glyph tone={tone} size={8} hollow={t.status === "BLOCKED"} />
                      </span>
                      <div className="min-w-0 leading-[1.5]">
                        <div className="flex items-baseline gap-2">
                          <span className={`min-w-0 flex-1 ${t.status === "COMPLETED" ? "text-(--a-muted)" : "font-medium"}`}>
                            {t.title}
                          </span>
                          <span className="shrink-0 text-[12px]" style={{ color: toneColor[tone] }}>
                            {statusLabel[t.status]}
                          </span>
                        </div>
                        <div className="mono flex gap-2 text-[11.5px] text-(--a-faint)">
                          <span>{t.id}</span>
                          <span>{t.owner}</span>
                          <span>
                            {t.startedAt}
                            {t.finishedAt ? `–${t.finishedAt}` : ""}
                          </span>
                          <span className="ml-auto">{duration(t.startedAt, t.finishedAt)}{t.finishedAt ? "" : " 経過"}</span>
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ol>
            </section>

            <section className="border-b border-(--a-line) px-4 py-3.5">
              <SectionTitle aside={`${agents.length}`}>参加者</SectionTitle>
              <ul className="m-0 list-none p-0">
                {agents.map((a) => (
                  <li key={a.id} className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 py-1.5 leading-[1.5]">
                    <span className="flex items-center gap-2">
                      <span className="font-medium">{a.name}</span>
                      <span className="mono text-[11.5px] text-(--a-faint)">{nodeLabel(a.node)}</span>
                    </span>
                    <span className="flex items-center gap-1.5 text-[12px]" style={{ color: toneColor[statusTone(a.phase)] }}>
                      {a.phase === "THINKING" ? <span className="a-pulse" /> : <Glyph tone={statusTone(a.phase)} size={6} />}
                      {statusLabel[a.phase]}
                    </span>
                    <span className="mono col-span-2 text-[11.5px] text-(--a-muted)">{a.model}</span>
                  </li>
                ))}
              </ul>
            </section>

            <section className="px-4 py-3.5">
              <SectionTitle aside={`${artifacts.length}`}>成果物</SectionTitle>
              <ul className="m-0 flex list-none flex-col gap-3 p-0">
                {artifacts.map((a) => (
                  <li key={a.id}>
                    <div className="flex items-baseline gap-2">
                      <span className="mono min-w-0 flex-1 truncate text-[12.5px] font-medium">{a.name}</span>
                      <span className="mono text-[11.5px] text-(--a-faint)">{a.size}</span>
                    </div>
                    <div className="mono text-[11.5px] text-(--a-faint)">
                      {a.by} · {a.createdAt}
                    </div>
                    <div className="mono mt-1.5 border-l-2 border-(--a-line) pl-2.5 text-[11.5px] leading-[1.7] text-(--a-muted)">
                      {a.preview.map((line) => (
                        <div key={line} className="truncate">
                          {line}
                        </div>
                      ))}
                    </div>
                  </li>
                ))}
              </ul>
            </section>
          </>
        ) : (
          <section className="px-4 py-3.5">
            <SectionTitle>参加エージェント</SectionTitle>
            <p className="mono m-0 text-[12px] text-(--a-muted)">{workspace.agents.join(", ")}</p>
          </section>
        )}
      </div>
    </Pane>
  );
}

