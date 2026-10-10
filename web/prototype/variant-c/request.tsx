// PROTOTYPE ONLY. Variant C request screen: the request as a document, conversation as marginalia.
import { useLayoutEffect, useRef, useState } from "react";
import { ArrowUp, Check, Ellipsis, FileDiff, FileText, Paperclip, Share2 } from "lucide-react";
import {
  agentName,
  agents,
  artifacts,
  humanRequest,
  messages,
  statusLabel,
  tasks,
  viewer,
  workspaces,
  type Message,
} from "../data";
import type { Screen } from "../types";
import { Avatar, NOW, Prose, StatusWord, TopStrip, nodeName, toMinutes } from "./parts";

type SectionKey = "intro" | "decision" | "plan" | "artifacts" | "log";

const threads: { key: SectionKey; label: string; ids: string[] }[] = [
  { key: "intro", label: "依頼について", ids: ["m1"] },
  { key: "decision", label: "判断について", ids: ["m7"] },
  { key: "plan", label: "計画について", ids: ["m2", "m5", "m6"] },
  { key: "artifacts", label: "成果物について", ids: ["m3", "m4"] },
];

const workspace = workspaces[0];
const nodeNames = [...new Set(workspace.agents.map((id) => nodeName(agents.find((a) => a.id === id)!.node)))];

type Decision =
  | { state: "open" }
  | { state: "approved" | "rejected"; at: string }
  | { state: "drafting" }
  | { state: "replied"; at: string; text: string };

export function RequestScreen({ onScreen }: { onScreen: (s: Screen) => void }) {
  const [decision, setDecision] = useState<Decision>({ state: "open" });
  const [draft, setDraft] = useState("");
  const [added, setAdded] = useState<Message[]>([]);

  const sectionRefs = useRef<Partial<Record<SectionKey, HTMLElement | null>>>({});
  const threadRefs = useRef<Record<string, HTMLDivElement | null>>({});
  const marginRef = useRef<HTMLDivElement>(null);
  const docRef = useRef<HTMLElement>(null);
  const [layout, setLayout] = useState<{ tops: Record<string, number>; height: number }>({ tops: {}, height: 0 });

  const allThreads = added.length
    ? [...threads, { key: "log" as SectionKey, label: "追加した指示", ids: added.map((m) => m.id) }]
    : threads;

  // Place each margin thread beside its section, pushing down to avoid overlap (Docs-style marginalia).
  useLayoutEffect(() => {
    const margin = marginRef.current;
    const doc = docRef.current;
    if (!margin || !doc) return;
    const place = () => {
      const base = margin.getBoundingClientRect().top;
      const tops: Record<string, number> = {};
      let cursor = 0;
      for (const thread of allThreads) {
        const section = sectionRefs.current[thread.key];
        const el = threadRefs.current[thread.key];
        if (!section || !el) continue;
        const top = Math.max(section.getBoundingClientRect().top - base, cursor);
        tops[thread.key] = Math.round(top);
        cursor = top + el.offsetHeight + 28;
      }
      const height = Math.round(Math.max(doc.offsetHeight, cursor + 200));
      setLayout((prev) =>
        prev.height === height && JSON.stringify(prev.tops) === JSON.stringify(tops) ? prev : { tops, height },
      );
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(doc);
    for (const el of Object.values(threadRefs.current)) if (el) observer.observe(el);
    return () => observer.disconnect();
  }, [allThreads.length]);

  const done = tasks.filter((t) => t.status === "COMPLETED").length;

  const send = () => {
    const text = draft.trim();
    if (!text) return;
    setAdded((prev) => [...prev, { id: `local-${prev.length}`, sender: "human", at: NOW, content: text }]);
    setDraft("");
  };

  return (
    <>
      <TopStrip crumbs={[{ label: "依頼" }, { label: workspace.title }]} note={`${NOW} に更新`}>
        <button type="button" className="c-btn c-btn--quiet c-btn--sm" onClick={() => onScreen("observe")}>
          実行レポート
        </button>
        <button type="button" className="c-btn c-btn--sm">
          <Share2 size={14} />
          共有
        </button>
        <button type="button" className="c-icon-btn" aria-label="その他の操作">
          <Ellipsis size={16} />
        </button>
      </TopStrip>

      <div className="c-page">
        <article className="c-doc" ref={docRef}>
          <section ref={(el) => void (sectionRefs.current.intro = el)}>
            <div className="c-eyebrow">
              <span>依頼</span>
              <span aria-hidden>·</span>
              <span className="mono">{workspace.id}</span>
            </div>
            <h1 className="c-title">{workspace.title}</h1>
            <p className="c-lede">{workspace.goal}</p>
            <dl className="c-meta">
              <div>
                <dt>状態</dt>
                <dd>
                  <StatusWord status={workspace.status} />
                </dd>
              </div>
              <div>
                <dt>開始</dt>
                <dd className="mono">09:01</dd>
              </div>
              <div>
                <dt>エージェント</dt>
                <dd className="num">{workspace.agents.length}</dd>
              </div>
              <div>
                <dt>ノード</dt>
                <dd className="num">
                  {nodeNames.length}
                  <span className="muted font-normal"> ({nodeNames.join(", ")})</span>
                </dd>
              </div>
              <div>
                <dt>進捗</dt>
                <dd className="num">
                  {done} / {tasks.length}
                </dd>
              </div>
            </dl>
          </section>

          <section ref={(el) => void (sectionRefs.current.decision = el)}>
            <h2 className="c-h2">
              判断が必要です
              <small>1 件</small>
            </h2>
            <DecisionCallout decision={decision} setDecision={setDecision} />
          </section>

          <section ref={(el) => void (sectionRefs.current.plan = el)}>
            <h2 className="c-h2">
              計画
              <small className="num">
                {tasks.length} タスク中 {done} 完了
              </small>
            </h2>
            <ul className="c-tasks">
              {tasks.map((task) => {
                const running = task.status === "RUNNING";
                const blocked = task.status === "BLOCKED";
                const complete = task.status === "COMPLETED";
                const minutes = toMinutes(task.finishedAt ?? NOW) - toMinutes(task.startedAt);
                return (
                  <li key={task.id} className={`c-task${complete ? " c-task--done" : ""}`}>
                    <span
                      className={`c-box${complete ? " c-box--done" : running ? " c-box--running" : blocked ? " c-box--blocked" : ""}`}
                      role="img"
                      aria-label={statusLabel[task.status]}
                    >
                      {complete && <Check size={11} strokeWidth={3} />}
                    </span>
                    <div className="min-w-0">
                      <div className="c-task__title">{task.title}</div>
                      <div className="c-task__sub">
                        <span className="mono">{task.id}</span>
                        <span>{agentName(task.owner)}</span>
                        {task.dependencies.length > 0 && <span>{task.dependencies.join("・")} の完了後</span>}
                        {blocked && <span className="tone-warn">公開には承認が必要</span>}
                      </div>
                    </div>
                    <div className="c-task__side">
                      <div>
                        <StatusWord status={task.status} />
                      </div>
                      <div className="num">
                        {complete
                          ? `${task.startedAt}–${task.finishedAt} · ${minutes}分`
                          : running
                            ? `${task.startedAt} から · ${minutes}分経過`
                            : "承認待ち"}
                      </div>
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>

          <section ref={(el) => void (sectionRefs.current.artifacts = el)}>
            <h2 className="c-h2">
              成果物
              <small className="num">{artifacts.length} 件</small>
            </h2>
            <ArtifactEmbeds />
          </section>

          <section ref={(el) => void (sectionRefs.current.log = el)}>
            <h2 className="c-h2">経過</h2>
            <dl className="c-log">
              <dt>09:01</dt>
              <dd>
                <strong>{viewer.displayName}</strong> が依頼を作成しました。lab ノードの Verifier を使い、障害時の復旧まで確認する方針です。
              </dd>
              <dt>09:11</dt>
              <dd>
                <strong>Researcher</strong> が前回リリースとの差分 212 ファイルを確認し、再接続時にトークンが再発行されない問題を見つけました。
              </dd>
              <dt>09:27</dt>
              <dd>
                <strong>Coder</strong> の修正中に handshake_reconnect が 3 回中 1 回失敗しました。<span className="tone-warn">注意</span>
              </dd>
              <dt>09:33</dt>
              <dd>
                <strong>Coder</strong> が修正を完了し、federation-fix.diff を残しました。cargo test は 148 件すべて通過しています。
              </dd>
              <dt>09:34</dt>
              <dd>
                作業が lab ノードへ移り、<strong>Verifier</strong> が復旧テストを引き受けました。
              </dd>
              <dt>09:38</dt>
              <dd>
                <strong>Publisher</strong> が公開の承認を求めています。復旧テストの完了後に実行されます。
              </dd>
              <dt>{NOW}</dt>
              <dd>
                Verifier が edge-02 の 3 回目の切断テストを実行中です。
              </dd>
            </dl>
          </section>
        </article>

        <aside className="c-margin" ref={marginRef} aria-label="コメント" style={{ minHeight: layout.height || undefined }}>
          {allThreads.map((thread) => (
            <div
              key={thread.key}
              className="c-thread"
              ref={(el) => void (threadRefs.current[thread.key] = el)}
              style={{ top: layout.tops[thread.key] ?? 0, visibility: thread.key in layout.tops ? "visible" : "hidden" }}
            >
              <div className="c-thread__anchor">{thread.label}</div>
              {thread.ids.map((id) => (
                <Comment key={id} message={[...messages, ...added].find((m) => m.id === id)!} />
              ))}
            </div>
          ))}
          <div className="c-composer-dock">
          <form
            className="c-composer"
            onSubmit={(event) => {
              event.preventDefault();
              send();
            }}
          >
            <label className="sr-only" htmlFor="c-compose">
              指示を追加
            </label>
            <textarea
              id="c-compose"
              className="c-textarea"
              rows={2}
              placeholder="指示を追加"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) send();
              }}
            />
            <div className="c-composer__foot">
              <span>@ でエージェントを指定</span>
              <button type="submit" className="c-btn c-btn--primary c-btn--sm" disabled={!draft.trim()}>
                <ArrowUp size={14} />
                送信
              </button>
            </div>
          </form>
          </div>
        </aside>
      </div>
    </>
  );
}

function DecisionCallout({ decision, setDecision }: { decision: Decision; setDecision: (d: Decision) => void }) {
  const [reply, setReply] = useState("");
  return (
    <div className="c-callout" role="group" aria-labelledby="c-decision-prompt">
      <div className="c-callout__from">
        <Avatar id={humanRequest.agent} />
        <span>
          <b className="font-bold text-[var(--c-ink)]">{agentName(humanRequest.agent)}</b> からの承認依頼
        </span>
        <span aria-hidden>·</span>
        <span className="mono">{humanRequest.taskId}</span>
        <span aria-hidden>·</span>
        <span className="mono">{humanRequest.createdAt}</span>
      </div>
      <p id="c-decision-prompt" className="c-callout__prompt">
        {humanRequest.prompt}
      </p>
      <p className="c-callout__detail">
        <Prose text={humanRequest.detail} />
      </p>

      {decision.state === "open" && (
        <div className="c-callout__actions">
          <button type="button" className="c-btn c-btn--primary" onClick={() => setDecision({ state: "approved", at: NOW })}>
            承認
          </button>
          <button type="button" className="c-btn" onClick={() => setDecision({ state: "rejected", at: NOW })}>
            却下
          </button>
          <button type="button" className="c-btn c-btn--quiet" onClick={() => setDecision({ state: "drafting" })}>
            条件付きで返信
          </button>
        </div>
      )}

      {decision.state === "drafting" && (
        <div className="mt-4">
          <textarea
            className="c-textarea"
            rows={2}
            autoFocus
            placeholder="例: 社内レジストリのみ先に公開してください"
            value={reply}
            onChange={(event) => setReply(event.target.value)}
          />
          <div className="c-callout__actions mt-2.5">
            <button
              type="button"
              className="c-btn c-btn--primary c-btn--sm"
              disabled={!reply.trim()}
              onClick={() => setDecision({ state: "replied", at: NOW, text: reply.trim() })}
            >
              条件を送る
            </button>
            <button type="button" className="c-btn c-btn--quiet c-btn--sm" onClick={() => setDecision({ state: "open" })}>
              取り消す
            </button>
          </div>
        </div>
      )}

      {(decision.state === "approved" || decision.state === "rejected" || decision.state === "replied") && (
        <div className="c-callout__resolved">
          <span className={decision.state === "rejected" ? "tone-error font-medium" : "tone-accent font-medium"}>
            {decision.state === "approved" ? "承認しました" : decision.state === "rejected" ? "却下しました" : "条件を返信しました"}
          </span>
          <span className="muted text-[13px]">
            {viewer.displayName} · <span className="mono">{decision.at}</span>
            {decision.state === "approved" && " · task-4 の完了後に公開されます"}
            {decision.state === "replied" && ` · 「${decision.text}」`}
          </span>
          <button type="button" className="c-link ml-auto text-[13px]" onClick={() => setDecision({ state: "open" })}>
            元に戻す
          </button>
        </div>
      )}
    </div>
  );
}

function ArtifactEmbeds() {
  const [checklist, diff] = artifacts;
  const diffRows = diff.preview.map((line) => {
    const [path, stat] = line.split(/\s{2,}/);
    const [add, del] = stat.split(" ");
    return { path, add, del };
  });
  const totals = diffRows.reduce(
    (sum, row) => ({ add: sum.add + Number(row.add.slice(1)), del: sum.del + Number(row.del.slice(1)) }),
    { add: 0, del: 0 },
  );
  return (
    <>
      <figure className="c-embed">
        <figcaption className="c-embed__head">
          <FileText size={15} strokeWidth={1.75} />
          <span className="c-embed__name">{checklist.name}</span>
          <span className="c-embed__by">
            {agentName(checklist.by)} · <span className="mono">{checklist.createdAt}</span> · {checklist.size}
          </span>
          <button type="button" className="c-btn c-btn--quiet c-btn--sm ml-auto">
            開く
          </button>
        </figcaption>
        <div className="c-embed__body c-snippet">
          <h4>リリースチェックリスト</h4>
          <ol>
            {checklist.preview.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ol>
        </div>
      </figure>
      <figure className="c-embed">
        <figcaption className="c-embed__head">
          <FileDiff size={15} strokeWidth={1.75} />
          <span className="c-embed__name">{diff.name}</span>
          <span className="c-embed__by">
            {agentName(diff.by)} · <span className="mono">{diff.createdAt}</span> · {diff.size}
          </span>
          <button type="button" className="c-btn c-btn--quiet c-btn--sm ml-auto">
            差分を見る
          </button>
        </figcaption>
        <div className="c-embed__body">
          {diffRows.map((row) => (
            <div key={row.path} className="c-diffrow">
              <span className="c-diffrow__path">{row.path}</span>
              <span className="num">
                <span className="c-diff-add">{row.add}</span> <span className="c-diff-del">{row.del}</span>
              </span>
            </div>
          ))}
        </div>
        <div className="c-embed__foot num">
          {diffRows.length} ファイルを変更 · <span className="c-diff-add">+{totals.add}</span>{" "}
          <span className="c-diff-del">−{totals.del}</span> · ハンドシェイクのテストを追加
        </div>
      </figure>
    </>
  );
}

function Comment({ message }: { message: Message }) {
  return (
    <div className={`c-cmt${message.sender === "system" ? " c-cmt--system" : ""}`}>
      <div className="c-cmt__head">
        <Avatar id={message.sender} />
        <span className="c-cmt__name">{agentName(message.sender)}</span>
        <span className="c-cmt__time">{message.at}</span>
      </div>
      <p className="c-cmt__body">
        <Prose text={message.content} />
      </p>
      {message.attachment && (
        <button type="button" className="c-chip">
          <Paperclip size={11} />
          {message.attachment}
        </button>
      )}
      {message.thread && (
        <div className="c-cmt__replies">
          <button type="button" className="c-link">
            返信 {message.thread} 件
          </button>
        </div>
      )}
    </div>
  );
}
