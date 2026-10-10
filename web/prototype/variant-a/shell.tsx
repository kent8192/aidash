// PROTOTYPE ONLY. Top bar, command palette, menus and status line for variant A.
import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowRight,
  Bell,
  Blocks,
  Bot,
  Boxes,
  Check,
  ChevronDown,
  ChevronRight,
  CircleUserRound,
  Command,
  Eye,
  FileStack,
  Globe,
  Inbox,
  KeyRound,
  LogOut,
  Network,
  PenTool,
  Plus,
  Rocket,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Store,
  SunMedium,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { humanRequest, incidents, node, peers, viewer, workspaces } from "../data";
import type { Screen } from "../types";
import { Glyph, Kbd, nodeLabel } from "./ui";

export const destinations: { key: Screen | "create"; label: string; hint: string }[] = [
  { key: "request", label: "依頼", hint: "1" },
  { key: "observe", label: "観測", hint: "2" },
  { key: "govern", label: "統制", hint: "3" },
  { key: "create", label: "作成", hint: "4" },
];

type Command = {
  id: string;
  group: string;
  label: string;
  detail?: string;
  icon: LucideIcon;
  keys?: string[];
  run?: () => void;
};

export function TopBar({
  screen,
  onScreen,
  onPalette,
}: {
  screen: Screen;
  onScreen: (s: Screen) => void;
  onPalette: () => void;
}) {
  const [menu, setMenu] = useState<"account" | "notifications" | null>(null);
  const openIncidents = incidents.filter((i) => i.status === "open");
  const notificationCount = 1 + openIncidents.length;

  return (
    <header className="relative z-20 grid h-12 shrink-0 grid-cols-[1fr_minmax(280px,420px)_1fr] items-center gap-4 border-b border-(--a-line) bg-(--a-surface) px-4">
      <div className="flex min-w-0 items-center gap-5">
        <span className="text-[15px] font-semibold tracking-[-0.02em]" style={{ fontFamily: "Geist" }}>
          aidash
        </span>
        <nav aria-label="主要な移動先" className="flex items-center">
          {destinations.map((d) => (
            <button
              key={d.key}
              type="button"
              className="a-tab"
              aria-current={d.key === screen ? "page" : undefined}
              title={d.key === "create" ? "Creator ワークベンチ (このプロトタイプでは非表示)" : undefined}
              onClick={() => d.key !== "create" && onScreen(d.key)}
            >
              {d.label}
            </button>
          ))}
        </nav>
      </div>

      <button
        type="button"
        onClick={onPalette}
        className="group flex h-8 w-full items-center gap-2 rounded-(--a-r-control) border border-(--a-line) bg-(--a-bg) px-2.5 text-left text-(--a-faint) transition-colors hover:border-[#d0d5dd] hover:bg-(--a-surface)"
      >
        <Search size={14} strokeWidth={2} className="shrink-0" />
        <span className="flex-1 truncate text-[13px]">コマンドまたは検索</span>
        <span className="flex gap-1">
          <Kbd>⌘</Kbd>
          <Kbd>K</Kbd>
        </span>
      </button>

      <div className="flex items-center justify-end gap-1">
        <span className="mr-2 flex items-center gap-2 text-[12px] text-(--a-muted)" title="イベントストリーム">
          <span className="a-pulse text-(--a-done)" />
          接続中
        </span>
        <div className="relative">
          <button
            type="button"
            className="a-icon-btn relative"
            aria-label={`通知 ${notificationCount}件`}
            aria-expanded={menu === "notifications"}
            onClick={() => setMenu(menu === "notifications" ? null : "notifications")}
          >
            <Bell size={16} strokeWidth={1.8} />
            <span className="mono absolute -top-0.5 -right-0.5 grid h-[15px] min-w-[15px] place-items-center rounded-[4px] bg-(--a-accent) px-[3px] text-[10px] leading-none font-medium text-white">
              {notificationCount}
            </span>
          </button>
          {menu === "notifications" ? (
            <Popover onClose={() => setMenu(null)} className="right-0 w-[360px]">
              <div className="flex items-center justify-between px-3 pt-2.5 pb-1.5">
                <span className="a-eyebrow">通知</span>
                <button type="button" className="a-btn a-btn-ghost h-6 px-1.5">
                  すべて既読
                </button>
              </div>
              <ul className="m-0 list-none p-1.5 pt-0">
                <li>
                  <button
                    type="button"
                    className="a-menu-item h-auto items-start py-2"
                    onClick={() => {
                      setMenu(null);
                      onScreen("request");
                    }}
                  >
                    <span className="mt-[7px]">
                      <Glyph tone="warn" />
                    </span>
                    <span className="min-w-0 flex-1 leading-[1.5]">
                      <span className="block">{humanRequest.prompt}</span>
                      <span className="mono block text-[12px] text-(--a-muted)">
                        publisher · v0.1-release · {humanRequest.createdAt}
                      </span>
                    </span>
                  </button>
                </li>
                {openIncidents.map((i) => (
                  <li key={i.id}>
                    <button
                      type="button"
                      className="a-menu-item h-auto items-start py-2"
                      onClick={() => {
                        setMenu(null);
                        onScreen("govern");
                      }}
                    >
                      <span className="mt-[7px]">
                        <Glyph tone={i.severity === "high" ? "fail" : "warn"} />
                      </span>
                      <span className="min-w-0 flex-1 leading-[1.5]">
                        <span className="block">{i.title}</span>
                        <span className="mono block text-[12px] text-(--a-muted)">
                          {i.id} · {i.severity} · {i.openedAt}
                        </span>
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </Popover>
          ) : null}
        </div>
        <div className="relative ml-1">
          <button
            type="button"
            aria-expanded={menu === "account"}
            onClick={() => setMenu(menu === "account" ? null : "account")}
            className="mono flex h-7 items-center gap-1.5 rounded-(--a-r-control) border border-(--a-line) px-2 text-[12px] text-(--a-text) transition-colors hover:bg-(--a-sunken)"
          >
            <KeyRound size={13} strokeWidth={1.8} className="text-(--a-muted)" />
            {viewer.authority}
            <ChevronDown size={13} className="text-(--a-muted)" />
          </button>
          {menu === "account" ? (
            <AccountMenu onClose={() => setMenu(null)} />
          ) : null}
        </div>
      </div>
    </header>
  );
}

function Popover({
  children,
  onClose,
  className = "",
}: {
  children: React.ReactNode;
  onClose: () => void;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.parentElement?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);
  return (
    <div ref={ref} role="menu" className={`a-overlay absolute top-[calc(100%+6px)] z-30 ${className}`}>
      {children}
    </div>
  );
}

function AccountMenu({ onClose }: { onClose: () => void }) {
  const authorities = [
    { id: viewer.authority, node: node.name, current: true },
    ...peers.map((p) => ({ id: `${p.name} / mika.tanaka`, node: p.name, current: false })),
  ];
  return (
    <Popover onClose={onClose} className="right-0 w-[300px]">
      <div className="flex items-center gap-2.5 border-b border-(--a-line) px-3 py-3">
        <CircleUserRound size={28} strokeWidth={1.4} className="text-(--a-muted)" />
        <div className="min-w-0 leading-[1.4]">
          <div className="font-medium">{viewer.displayName}</div>
          <div className="mono truncate text-[12px] text-(--a-muted)">{viewer.subject}</div>
        </div>
      </div>
      <div className="p-1.5">
        <div className="a-eyebrow px-2.5 pt-1 pb-1">権限の切り替え</div>
        {authorities.map((a) => (
          <button key={a.id} type="button" className="a-menu-item">
            <span className="mono flex-1 truncate text-[12px]">{a.id}</span>
            {a.current ? <Check size={14} className="text-(--a-accent)" /> : null}
          </button>
        ))}
      </div>
      <div className="border-t border-(--a-line) p-1.5">
        <MenuRow icon={Globe} label="言語" value="日本語" />
        <MenuRow icon={SunMedium} label="テーマ" value="ライト" />
        <MenuRow icon={Settings} label="設定" keys="⌘ ," />
      </div>
      <div className="border-t border-(--a-line) p-1.5">
        <MenuRow icon={LogOut} label="サインアウト" />
      </div>
    </Popover>
  );
}

function MenuRow({ icon: Icon, label, value, keys }: { icon: LucideIcon; label: string; value?: string; keys?: string }) {
  return (
    <button type="button" className="a-menu-item">
      <Icon size={15} strokeWidth={1.8} className="text-(--a-muted)" />
      <span className="flex-1">{label}</span>
      {value ? (
        <span className="flex items-center gap-1 text-[12px] text-(--a-muted)">
          {value}
          <ChevronRight size={13} />
        </span>
      ) : null}
      {keys ? <span className="mono text-[11px] text-(--a-faint)">{keys}</span> : null}
    </button>
  );
}

export function CommandPalette({
  onClose,
  onScreen,
  onApprove,
}: {
  onClose: () => void;
  onScreen: (s: Screen) => void;
  onApprove: () => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const commands = useMemo<Command[]>(() => {
    const go = (s: Screen) => () => {
      onScreen(s);
      onClose();
    };
    return [
      { id: "go-request", group: "移動", label: "依頼を開く", icon: Inbox, keys: ["1"], run: go("request") },
      { id: "go-observe", group: "移動", label: "観測を開く", icon: Eye, keys: ["2"], run: go("observe") },
      { id: "go-govern", group: "移動", label: "統制を開く", icon: ShieldCheck, keys: ["3"], run: go("govern") },
      { id: "go-create", group: "移動", label: "作成ワークベンチを開く", detail: "Creator", icon: PenTool, keys: ["4"] },
      {
        id: "approve",
        group: "操作",
        label: "パッケージの公開を承認",
        detail: "publisher · task-5",
        icon: Check,
        keys: ["A"],
        run: () => {
          onApprove();
          onScreen("request");
          onClose();
        },
      },
      { id: "new", group: "操作", label: "新しい依頼を作成", icon: Plus, keys: ["N"] },
      { id: "s-node", group: "設定", label: "ノード", detail: node.id, icon: Server },
      { id: "s-agents", group: "設定", label: "エージェント", detail: "4", icon: Bot },
      { id: "s-registry", group: "設定", label: "レジストリ", icon: FileStack },
      { id: "s-clusters", group: "設定", label: "クラスター", icon: Network },
      { id: "s-deploy", group: "設定", label: "配備", icon: Rocket },
      { id: "s-market", group: "設定", label: "マーケットプレイス", icon: Store },
      ...workspaces.map((w) => ({
        id: `ws-${w.id}`,
        group: "依頼",
        label: w.title,
        detail: `${w.tasksDone}/${w.tasksTotal}`,
        icon: ArrowRight,
        run: go("request"),
      })),
      { id: "p-lang", group: "表示", label: "言語を切り替え", detail: "日本語", icon: Globe },
      { id: "p-theme", group: "表示", label: "テーマを切り替え", detail: "ライト", icon: SunMedium },
      { id: "p-auth", group: "表示", label: "権限を切り替え", detail: viewer.authority, icon: KeyRound },
    ];
  }, [onClose, onScreen, onApprove]);

  const filtered = commands.filter((c) =>
    `${c.group} ${c.label} ${c.detail ?? ""}`.toLowerCase().includes(query.trim().toLowerCase()),
  );
  const groups = [...new Set(filtered.map((c) => c.group))];

  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => setActive(0), [query]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      filtered[active]?.run?.();
    } else if (e.key === "Escape") {
      onClose();
    }
  };

  return (
    <div className="fixed inset-0 z-40 bg-[rgb(15_20_30/0.08)]" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-label="コマンド"
        className="a-overlay absolute top-[64px] left-1/2 w-[600px] max-w-[calc(100vw-32px)] -translate-x-1/2 overflow-hidden"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <div className="flex h-12 items-center gap-2.5 border-b border-(--a-line) px-4">
          <Command size={15} className="text-(--a-muted)" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="コマンド、依頼、設定を検索"
            className="h-full flex-1 border-0 bg-transparent text-[14px] text-(--a-text) outline-none"
          />
          <Kbd>esc</Kbd>
        </div>
        <div className="a-scroll max-h-[min(500px,calc(100dvh-180px))] p-1.5">
          {groups.length === 0 ? (
            <p className="m-0 px-3 py-6 text-center text-(--a-muted)">一致するコマンドはありません</p>
          ) : null}
          {groups.map((g) => (
            <div key={g} className="pb-1">
              <div className="a-eyebrow px-2.5 pt-2 pb-1">{g}</div>
              {filtered
                .filter((c) => c.group === g)
                .map((c) => {
                  const index = filtered.indexOf(c);
                  return (
                    <button
                      key={c.id}
                      type="button"
                      className="a-menu-item"
                      data-active={index === active}
                      onMouseEnter={() => setActive(index)}
                      onClick={() => c.run?.()}
                    >
                      <c.icon size={15} strokeWidth={1.8} className="text-(--a-muted)" />
                      <span className={g === "依頼" ? "mono text-[12.5px]" : ""}>{c.label}</span>
                      {c.detail ? (
                        <span className="mono truncate text-[12px] text-(--a-faint)">{c.detail}</span>
                      ) : null}
                      <span className="flex-1" />
                      {c.keys?.map((k) => <Kbd key={k}>{k}</Kbd>)}
                    </button>
                  );
                })}
            </div>
          ))}
        </div>
        <div className="flex h-9 items-center gap-4 border-t border-(--a-line) bg-(--a-bg) px-4 text-[12px] text-(--a-muted)">
          <span className="flex items-center gap-1.5">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd>
            移動
          </span>
          <span className="flex items-center gap-1.5">
            <Kbd>↵</Kbd>
            実行
          </span>
          <span className="flex-1" />
          <span className="flex items-center gap-1.5">
            <Boxes size={13} />
            {filtered.length} 件
          </span>
        </div>
      </div>
    </div>
  );
}

export function StatusLine({ hints }: { hints: [string, string][] }) {
  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-(--a-line) bg-(--a-surface) px-4 text-[12px] text-(--a-muted)">
      <span className="mono flex items-center gap-1.5">
        <Glyph tone="done" size={6} />
        {node.id}
      </span>
      {peers.map((p) => (
        <span key={p.id} className="mono flex items-center gap-1.5">
          <Glyph tone={p.status === "connected" ? "done" : "warn"} size={6} />
          {nodeLabel(p.id)}
          <span className={p.status === "connected" ? "text-(--a-faint)" : "text-(--a-warn)"}>
            {p.latencyMs}ms
          </span>
        </span>
      ))}
      <span className="flex-1" />
      <span className="flex items-center gap-3">
        {hints.map(([k, label]) => (
          <span key={label} className="flex items-center gap-1.5">
            <Kbd>{k}</Kbd>
            {label}
          </span>
        ))}
        <span className="flex items-center gap-1.5">
          <Blocks size={13} />
          v0.1.0
        </span>
      </span>
    </footer>
  );
}
