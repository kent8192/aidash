// PROTOTYPE ONLY. Variant B shell: icon rail, context bar with workspace switcher and decision queue.
import { useEffect, useState, type ReactNode } from "react";
import {
  Bell,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Languages,
  LogOut,
  MessagesSquare,
  Network,
  Package,
  PencilRuler,
  Plus,
  Radar,
  Rocket,
  Server,
  Settings2,
  ShieldHalf,
  Store,
  Moon,
  Sun,
  SunMoon,
  UserRoundCog,
} from "lucide-react";
import {
  agentName,
  agents,
  events,
  humanRequest,
  incidents,
  node,
  peers,
  statusLabel,
  viewer,
  workspaces,
} from "../data";
import type { Screen } from "../types";
import { phaseTone, type Decision } from "./shared";
import { RequestScreen } from "./request";
import { ObserveScreen } from "./observe";
import { GovernScreen } from "./govern";

type Menu = "settings" | "account" | "notify" | "workspace" | "queue" | null;
type ThemePref = "dark" | "light" | "system";
type Theme = "dark" | "light";

const THEME_KEY = "proto-b-theme";
const darkQuery = "(prefers-color-scheme: dark)";

// Theme preference persisted locally; "system" follows the OS setting live.
function useTheme() {
  const [pref, setPrefState] = useState<ThemePref>(() => {
    const stored = localStorage.getItem(THEME_KEY);
    return stored === "light" || stored === "system" || stored === "dark" ? stored : "dark";
  });
  const [systemDark, setSystemDark] = useState(() => matchMedia(darkQuery).matches);
  useEffect(() => {
    const query = matchMedia(darkQuery);
    const onChange = (event: MediaQueryListEvent) => setSystemDark(event.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  const setPref = (next: ThemePref) => {
    localStorage.setItem(THEME_KEY, next);
    setPrefState(next);
  };
  const theme: Theme = pref === "system" ? (systemDark ? "dark" : "light") : pref;
  return { pref, setPref, theme };
}

const nav: { key: Screen; label: string; icon: typeof Radar; kbd: string }[] = [
  { key: "request", label: "依頼", icon: MessagesSquare, kbd: "1" },
  { key: "observe", label: "観測", icon: Radar, kbd: "2" },
  { key: "govern", label: "統制", icon: ShieldHalf, kbd: "3" },
];
const screenLabel: Record<Screen, string> = { request: "依頼", observe: "観測", govern: "統制" };

const notices = [
  ...events
    .filter((e) => e.severity)
    .reverse()
    .map((e) => ({ at: e.at.slice(0, 5), who: agentName(e.actor), text: e.text, tone: e.kind === "request" ? "warn" : "idle" })),
  ...incidents
    .filter((i) => i.status === "open" && i.severity === "high")
    .map((i) => ({ at: i.openedAt.slice(-5), who: i.id, text: i.title, tone: "error" })),
].sort((a, b) => b.at.localeCompare(a.at));

const settings = [
  { label: "ノード", meta: node.id, icon: Server },
  { label: "エージェント", meta: `${agents.length}`, icon: Bot },
  { label: "レジストリ", meta: "", icon: Package },
  { label: "クラスター", meta: `${peers.length + 1} ノード`, icon: Network },
  { label: "配備", meta: "", icon: Rocket },
  { label: "マーケットプレイス", meta: "", icon: Store },
];

function RailButton({
  label,
  kbd,
  current,
  onClick,
  children,
  expanded,
}: {
  label: string;
  kbd?: string;
  current?: boolean;
  onClick?: () => void;
  children: ReactNode;
  expanded?: boolean;
}) {
  return (
    <button
      type="button"
      className="b-railbtn"
      aria-label={label}
      aria-current={current ? "page" : undefined}
      aria-expanded={expanded}
      onClick={onClick}
    >
      {children}
      {!expanded && (
        <span className="b-tip" role="tooltip">
          {label}
          {kbd && <span className="b-kbd">{kbd}</span>}
        </span>
      )}
    </button>
  );
}

function DecisionBody({
  decision,
  setDecision,
  compact,
}: {
  decision: Decision;
  setDecision: (d: Decision) => void;
  compact?: boolean;
}) {
  if (decision !== "pending") {
    return (
      <div className="b-decided" data-decision={decision}>
        <Check size={14} />
        <span>
          {decision === "approved" ? "承認済み" : "却下済み"}
          <span className="b-muted"> · 09:41 {viewer.displayName}</span>
        </span>
        <button type="button" className="b-link" onClick={() => setDecision("pending")}>
          取り消す
        </button>
      </div>
    );
  }
  return (
    <div className="b-actions">
      <button type="button" className="b-btn b-btn-primary" onClick={() => setDecision("approved")}>
        <Check size={14} />
        承認する
      </button>
      <button type="button" className="b-btn" onClick={() => setDecision("rejected")}>
        却下
      </button>
      {!compact && (
        <button type="button" className="b-btn b-btn-ghost">
          条件を付けて承認
        </button>
      )}
    </div>
  );
}

export function DecisionCard({
  decision,
  setDecision,
  className = "",
  style,
}: {
  decision: Decision;
  setDecision: (d: Decision) => void;
  className?: string;
  style?: React.CSSProperties;
}) {
  return (
    <div className={`b-decision ${className}`} data-decision={decision} style={style}>
      <div className="b-decision-kicker">
        <span className="b-dot" />
        <span>承認が必要</span>
        <span className="mono b-faint">{humanRequest.id}</span>
      </div>
      <p className="b-decision-title">{humanRequest.prompt}</p>
      <p className="b-decision-detail">{humanRequest.detail}</p>
      <dl className="b-decision-meta">
        <div>
          <dt>依頼元</dt>
          <dd>
            {agentName(humanRequest.agent)} <span className="mono b-faint">{humanRequest.taskId}</span>
          </dd>
        </div>
        <div>
          <dt>待機</dt>
          <dd className="mono">{humanRequest.createdAt} から 3m</dd>
        </div>
        <div>
          <dt>実行条件</dt>
          <dd>
            task-4 完了後 <span className="b-tag" data-tone="live">実行中</span>
          </dd>
        </div>
      </dl>
      <DecisionBody decision={decision} setDecision={setDecision} compact />
    </div>
  );
}

function ContextBar({
  screen,
  onScreen,
  decision,
  setDecision,
  menu,
  setMenu,
  theme,
  onToggleTheme,
}: {
  screen: Screen;
  onScreen: (s: Screen) => void;
  decision: Decision;
  setDecision: (d: Decision) => void;
  menu: Menu;
  setMenu: (m: Menu) => void;
  theme: Theme;
  onToggleTheme: () => void;
}) {
  const ws = workspaces[0];
  const pending = decision === "pending" ? 1 : 0;
  return (
    <header className="b-ctx">
      <nav className="b-crumbs" aria-label="現在地">
        <button type="button" className="b-crumb">
          <Server size={13} />
          <span className="mono">{node.name}</span>
        </button>
        <ChevronRight size={13} className="b-faint" />
        <div className="relative">
          <button
            type="button"
            className="b-crumb b-crumb-ws"
            aria-expanded={menu === "workspace"}
            onClick={() => setMenu(menu === "workspace" ? null : "workspace")}
          >
            <span className="b-dot" data-tone="live" />
            <span className="mono">{ws.title}</span>
            <ChevronDown size={13} className="b-faint" />
          </button>
          {menu === "workspace" && (
            <div className="b-fly b-fly-down" style={{ width: 380 }}>
              <div className="b-fly-head">
                <span>依頼を切り替え</span>
                <span className="b-kbd">⌘K</span>
              </div>
              {workspaces.map((w) => (
                <button
                  key={w.id}
                  type="button"
                  className="b-ws"
                  aria-current={w.id === ws.id}
                  onClick={() => setMenu(null)}
                >
                  <span className="b-dot" data-tone={phaseTone[w.status]} />
                  <span className="min-w-0">
                    <span className="flex items-center gap-2">
                      <span className="mono b-ws-title">{w.title}</span>
                      {w.pending > 0 && <span className="b-tag" data-tone="warn">判断待ち {w.pending}</span>}
                    </span>
                    <span className="b-ws-goal">{w.goal}</span>
                  </span>
                  <span className="b-ws-meta mono">
                    {w.tasksDone}/{w.tasksTotal}
                    <span className="b-faint">{statusLabel[w.status]}</span>
                  </span>
                </button>
              ))}
              <div className="b-fly-foot">
                <button type="button" className="b-mi">
                  <Plus size={14} />
                  新しい依頼
                </button>
              </div>
            </div>
          )}
        </div>
        <ChevronRight size={13} className="b-faint" />
        <span className="b-crumb-here">{screenLabel[screen]}</span>
      </nav>

      <div className="b-ctx-right">
        <div className="b-peers" aria-label="フェデレーション">
          {peers.map((p) => (
            <span key={p.id} className="b-peer" data-tone={p.status === "connected" ? "done" : "warn"}>
              <span className="b-dot" />
              <span className="mono">{p.name}</span>
              <span className="mono b-faint">{p.latencyMs}ms</span>
            </span>
          ))}
        </div>
        <span className="b-sep" />
        <span className="b-clock">
          <span className="b-live-dot" />
          <span className="mono">LIVE</span>
          <span className="mono b-muted">09:41</span>
        </span>
        <button
          type="button"
          className="b-ib"
          aria-label={theme === "dark" ? "ライトテーマに切り替える" : "ダークテーマに切り替える"}
          title={theme === "dark" ? "ライトテーマ" : "ダークテーマ"}
          onClick={onToggleTheme}
        >
          {theme === "dark" ? <Sun size={15} strokeWidth={1.75} /> : <Moon size={15} strokeWidth={1.75} />}
        </button>
        <div className="relative">
          <button
            type="button"
            className="b-queue"
            data-empty={pending === 0}
            aria-expanded={menu === "queue"}
            onClick={() => setMenu(menu === "queue" ? null : "queue")}
          >
            判断待ち
            <span className="b-queue-count mono">{pending}</span>
          </button>
          {menu === "queue" && (
            <div className="b-fly b-fly-down b-fly-right" style={{ width: 360 }}>
              <div className="b-fly-head">
                <span>判断キュー</span>
                <span className="mono b-faint">{pending} 件</span>
              </div>
              <div className="b-queue-item">
                <div className="b-decision-kicker">
                  <span className="b-dot" />
                  <span>承認が必要</span>
                  <span className="mono b-faint">
                    {agentName(humanRequest.agent)} · {humanRequest.taskId} · {humanRequest.createdAt}
                  </span>
                </div>
                <p className="b-decision-title">{humanRequest.prompt}</p>
                <p className="b-decision-detail">{humanRequest.detail}</p>
                <DecisionBody decision={decision} setDecision={setDecision} compact />
              </div>
              <div className="b-fly-foot">
                <button
                  type="button"
                  className="b-mi"
                  onClick={() => {
                    onScreen("request");
                    setMenu(null);
                  }}
                >
                  <MessagesSquare size={14} />
                  キャンバスで文脈を見る
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </header>
  );
}

function Rail({
  screen,
  onScreen,
  menu,
  setMenu,
  themePref,
  setThemePref,
}: {
  screen: Screen;
  onScreen: (s: Screen) => void;
  menu: Menu;
  setMenu: (m: Menu) => void;
  themePref: ThemePref;
  setThemePref: (pref: ThemePref) => void;
}) {
  const [lang, setLang] = useState("ja");
  const toggle = (m: Menu) => setMenu(menu === m ? null : m);
  return (
    <aside className="b-rail" aria-label="メインナビゲーション">
      <span className="b-mark mono" aria-label="Aidash">
        ad
      </span>
      {nav.map((n) => (
        <RailButton key={n.key} label={n.label} kbd={n.kbd} current={screen === n.key} onClick={() => onScreen(n.key)}>
          <n.icon size={18} strokeWidth={1.75} />
        </RailButton>
      ))}
      <span className="b-rail-sep" />
      <RailButton label="作成 · Creator ワークベンチ" kbd="4">
        <PencilRuler size={18} strokeWidth={1.75} />
      </RailButton>

      <div className="mt-auto flex flex-col items-center gap-1">
        <div className="relative">
          <RailButton label="通知" expanded={menu === "notify"} onClick={() => toggle("notify")}>
            <Bell size={18} strokeWidth={1.75} />
            <span className="b-badge mono">{notices.length}</span>
          </RailButton>
          {menu === "notify" && (
            <div className="b-fly b-fly-side" style={{ width: 340 }}>
              <div className="b-fly-head">
                <span>通知</span>
                <button type="button" className="b-link">
                  すべて既読にする
                </button>
              </div>
              {notices.map((n, i) => (
                <div key={i} className="b-notice" data-tone={n.tone}>
                  <span className="mono b-faint">{n.at}</span>
                  <span>
                    <span className="b-notice-who">{n.who}</span>
                    {n.text}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
        <div className="relative">
          <RailButton label="設定" expanded={menu === "settings"} onClick={() => toggle("settings")}>
            <Settings2 size={18} strokeWidth={1.75} />
          </RailButton>
          {menu === "settings" && (
            <div className="b-fly b-fly-side" style={{ width: 260 }}>
              <div className="b-fly-head">
                <span>設定</span>
                <span className="mono b-faint">{node.name}</span>
              </div>
              {settings.map((s) => (
                <button key={s.label} type="button" className="b-mi">
                  <s.icon size={15} strokeWidth={1.75} className="b-muted" />
                  <span className="flex-1">{s.label}</span>
                  {s.meta && <span className="mono b-faint">{s.meta}</span>}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="relative mt-2">
          <button
            type="button"
            className="b-me"
            aria-label="アカウント"
            aria-expanded={menu === "account"}
            onClick={() => toggle("account")}
          >
            田中
          </button>
          {menu === "account" && (
            <div className="b-fly b-fly-side" style={{ width: 300 }}>
              <div className="b-me-head">
                <span className="b-me b-me-lg">田中</span>
                <span className="min-w-0">
                  <span className="block font-medium">{viewer.displayName}</span>
                  <span className="mono b-faint block">{viewer.subject}</span>
                </span>
              </div>
              <div className="b-fly-group">
                <span className="b-fly-label">権限</span>
                <button type="button" className="b-mi" aria-current>
                  <UserRoundCog size={15} strokeWidth={1.75} className="b-muted" />
                  <span className="flex-1 mono">{viewer.authority}</span>
                  <Check size={14} className="b-accent" />
                </button>
                <button type="button" className="b-mi">
                  <span className="w-[15px]" />
                  <span className="flex-1 b-muted">権限を切り替える</span>
                </button>
              </div>
              <div className="b-fly-group">
                <span className="b-fly-label">
                  <Languages size={13} /> 言語
                </span>
                <div className="b-seg">
                  {[
                    ["ja", "日本語"],
                    ["en", "English"],
                  ].map(([k, l]) => (
                    <button key={k} type="button" aria-pressed={lang === k} onClick={() => setLang(k)}>
                      {l}
                    </button>
                  ))}
                </div>
              </div>
              <div className="b-fly-group">
                <span className="b-fly-label">
                  <SunMoon size={13} /> テーマ
                </span>
                <div className="b-seg">
                  {(
                    [
                      ["dark", "ダーク"],
                      ["light", "ライト"],
                      ["system", "システム"],
                    ] as const
                  ).map(([k, l]) => (
                    <button key={k} type="button" aria-pressed={themePref === k} onClick={() => setThemePref(k)}>
                      {l}
                    </button>
                  ))}
                </div>
              </div>
              <div className="b-fly-foot">
                <button type="button" className="b-mi">
                  <LogOut size={15} strokeWidth={1.75} className="b-muted" />
                  ログアウト
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </aside>
  );
}

export function Shell({ screen, onScreen }: { screen: Screen; onScreen: (s: Screen) => void }) {
  const [decision, setDecision] = useState<Decision>("pending");
  const [menu, setMenu] = useState<Menu>(null);
  const { pref, setPref, theme } = useTheme();

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenu(null);
      const target = event.target as HTMLElement;
      if (target.closest("input, textarea, select, [contenteditable]") || event.metaKey || event.ctrlKey) return;
      const next = nav.find((n) => n.kbd === event.key);
      if (next) onScreen(next.key);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onScreen]);

  return (
    <div className="proto-b" lang="ja" data-theme={theme}>
      <Rail
        screen={screen}
        onScreen={onScreen}
        menu={menu}
        setMenu={setMenu}
        themePref={pref}
        setThemePref={setPref}
      />
      <div className="b-main">
        <ContextBar
          screen={screen}
          onScreen={onScreen}
          decision={decision}
          setDecision={setDecision}
          menu={menu}
          setMenu={setMenu}
          theme={theme}
          onToggleTheme={() => setPref(theme === "dark" ? "light" : "dark")}
        />
        <main className="b-stage">
          {screen === "request" && <RequestScreen decision={decision} setDecision={setDecision} />}
          {screen === "observe" && <ObserveScreen />}
          {screen === "govern" && <GovernScreen decision={decision} />}
        </main>
      </div>
      {menu && <div className="b-scrim" onClick={() => setMenu(null)} />}
    </div>
  );
}
