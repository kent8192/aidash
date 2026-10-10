// PROTOTYPE ONLY. Variant C "Brief": each request is a living document; conversation lives in the margin.
import { useState, type ReactNode } from "react";
import {
  Activity,
  Bell,
  Boxes,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronsUpDown,
  FileText,
  Globe,
  KeyRound,
  Languages,
  LogOut,
  Network,
  PenLine,
  Plus,
  Rocket,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Store,
  Bot,
} from "lucide-react";
import { incidents, node, viewer, workspaces } from "../data";
import type { Screen, Variant } from "../types";
import { GovernScreen } from "./govern";
import { ObserveScreen } from "./observe";
import { RequestScreen } from "./request";
import "./c.css";

const primary: { key: Screen | "create"; label: string; icon: ReactNode }[] = [
  { key: "request", label: "依頼", icon: <FileText size={16} strokeWidth={1.75} /> },
  { key: "observe", label: "観測", icon: <Activity size={16} strokeWidth={1.75} /> },
  { key: "govern", label: "統制", icon: <ShieldCheck size={16} strokeWidth={1.75} /> },
  { key: "create", label: "作成", icon: <PenLine size={16} strokeWidth={1.75} /> },
];

const settingsItems: { label: string; icon: ReactNode }[] = [
  { label: "ノード", icon: <Server size={15} strokeWidth={1.75} /> },
  { label: "エージェント", icon: <Bot size={15} strokeWidth={1.75} /> },
  { label: "レジストリ", icon: <Boxes size={15} strokeWidth={1.75} /> },
  { label: "クラスター", icon: <Network size={15} strokeWidth={1.75} /> },
  { label: "配備", icon: <Rocket size={15} strokeWidth={1.75} /> },
  { label: "マーケットプレイス", icon: <Store size={15} strokeWidth={1.75} /> },
];

const statusWord: Record<string, { word: string; tone: string }> = {
  RUNNING: { word: "進行中", tone: "muted" },
  FAILED: { word: "失敗", tone: "tone-error" },
  COMPLETED: { word: "完了", tone: "muted" },
};

function Sidebar({ screen, onScreen }: { screen: Screen; onScreen: (s: Screen) => void }) {
  const [settingsOpen, setSettingsOpen] = useState(true);
  const [menuOpen, setMenuOpen] = useState(false);
  const [lang, setLang] = useState("ja");
  const [theme, setTheme] = useState("light");
  const notifications =
    workspaces.reduce((sum, w) => sum + w.pending, 0) +
    incidents.filter((i) => i.status === "open").length;

  return (
    <aside className="c-side" aria-label="ナビゲーション">
      <button type="button" className="c-switcher" aria-label="ワークスペースを切り替え">
        <span className="c-switcher__mark">a</span>
        <span className="min-w-0 flex-1">
          <span className="c-switcher__name block">{viewer.tenant}</span>
          <span className="c-switcher__sub block">
            {node.name} · {node.id}
          </span>
        </span>
        <ChevronsUpDown size={14} className="muted" />
      </button>
      <button type="button" className="c-search">
        <Search size={14} strokeWidth={1.75} />
        検索
        <span className="c-kbd">⌘K</span>
      </button>

      <div className="c-side__scroll">
        <nav className="c-nav" aria-label="主要">
          {primary.map((item) => (
            <button
              key={item.key}
              type="button"
              className="c-nav__item"
              aria-current={item.key === screen ? "page" : undefined}
              onClick={() => item.key !== "create" && onScreen(item.key)}
            >
              {item.icon}
              {item.label}
              {item.key === "create" && <span className="c-nav__trail">ワークベンチ</span>}
            </button>
          ))}
        </nav>

        <div className="c-side__label">
          <span>依頼</span>
          <button type="button" className="c-icon-btn" aria-label="新しい依頼">
            <Plus size={14} />
          </button>
        </div>
        <div className="c-nav">
          {workspaces.map((w, index) => {
            const status = statusWord[w.status];
            return (
              <button
                key={w.id}
                type="button"
                className="c-doclink"
                aria-current={index === 0 && screen === "request" ? "page" : undefined}
                onClick={() => onScreen("request")}
              >
                <FileText size={15} strokeWidth={1.75} />
                <span className="c-doclink__title">{w.title}</span>
                <span className="c-doclink__meta">
                  {w.pending > 0 && <span className="tone-warn">判断 {w.pending}</span>}
                  <span className={status.tone}>{status.word}</span>
                </span>
              </button>
            );
          })}
        </div>
      </div>

      <div className="c-side__foot">
        <div className="c-nav">
          <button
            type="button"
            className="c-nav__item"
            aria-expanded={settingsOpen}
            onClick={() => setSettingsOpen((v) => !v)}
          >
            <Settings size={16} strokeWidth={1.75} />
            設定
            <span className="c-nav__trail">
              {settingsOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            </span>
          </button>
          {settingsOpen && (
            <div className="c-subnav">
              {settingsItems.map((item) => (
                <button key={item.label} type="button" className="c-nav__item">
                  {item.icon}
                  {item.label}
                </button>
              ))}
            </div>
          )}
          <button type="button" className="c-nav__item">
            <Bell size={16} strokeWidth={1.75} />
            通知
            <span className="c-nav__trail num">{notifications} 件</span>
          </button>
        </div>

        <button
          type="button"
          className="c-account mt-1"
          aria-expanded={menuOpen}
          aria-haspopup="menu"
          onClick={() => setMenuOpen((v) => !v)}
        >
          <span className="c-avatar c-avatar--lg c-avatar--human">{viewer.displayName.charAt(0)}</span>
          <span className="min-w-0 flex-1">
            <span className="block text-[13.5px] font-medium leading-tight">{viewer.displayName}</span>
            <span className="block font-[family-name:var(--c-mono)] text-[11px] leading-snug text-[var(--c-muted)]">
              {viewer.authority}
            </span>
          </span>
        </button>
      </div>

      {menuOpen && (
        <div className="c-menu" role="menu">
          <div className="c-menu__label">権限</div>
          <button type="button" className="c-menu__item" role="menuitem">
            <KeyRound size={14} />
            <span className="mono">{viewer.authority}</span>
            <Check size={14} className="c-check" />
          </button>
          <button type="button" className="c-menu__item" role="menuitem">
            <ChevronsUpDown size={14} />
            権限を切り替える
          </button>
          <hr />
          <div className="c-menu__label flex items-center gap-1.5">
            <Languages size={13} /> 言語
          </div>
          <div className="c-seg">
            {[
              ["ja", "日本語"],
              ["en", "English"],
            ].map(([key, label]) => (
              <button key={key} type="button" aria-pressed={lang === key} onClick={() => setLang(key)}>
                {label}
              </button>
            ))}
            <span />
          </div>
          <div className="c-menu__label">テーマ</div>
          <div className="c-seg">
            {[
              ["light", "ライト"],
              ["dark", "ダーク"],
              ["system", "自動"],
            ].map(([key, label]) => (
              <button key={key} type="button" aria-pressed={theme === key} onClick={() => setTheme(key)}>
                {label}
              </button>
            ))}
          </div>
          <hr />
          <button type="button" className="c-menu__item" role="menuitem">
            <Globe size={14} />
            {viewer.subject}
          </button>
          <button type="button" className="c-menu__item" role="menuitem">
            <LogOut size={14} />
            ログアウト
          </button>
        </div>
      )}
    </aside>
  );
}

function Shell({ screen, onScreen }: { screen: Screen; onScreen: (s: Screen) => void }) {
  return (
    <div className="proto-c">
      <Sidebar screen={screen} onScreen={onScreen} />
      <main className="c-main" key={screen}>
        {screen === "request" && <RequestScreen onScreen={onScreen} />}
        {screen === "observe" && <ObserveScreen onScreen={onScreen} />}
        {screen === "govern" && <GovernScreen onScreen={onScreen} />}
      </main>
    </div>
  );
}

export const variantC: Variant = {
  name: "Brief",
  Component: Shell,
};
