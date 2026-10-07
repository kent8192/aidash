import type { ReactNode, RefObject } from "react";
import { Link } from "@tanstack/react-router";
import {
  Plus,
  Search,
  PanelLeftClose,
  ArrowLeft,
  MessageSquare,
} from "lucide-react";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { useI18n } from "./ui";
import type { State } from "./types";
import type { Destination } from "./collaboration/model";
import logo from "./assets/brand/aidash-logo.svg?no-inline";
import logoOnDark from "./assets/brand/aidash-logo-on-dark.svg?no-inline";

export function IntentSidebar({
  data,
  current,
  section,
  theme,
  filter,
  setFilter,
  searchInput,
  expanded,
  close,
  create,
  account,
}: {
  data?: State;
  current: string;
  section: Destination;
  theme: string;
  filter: string;
  setFilter: (value: string) => void;
  searchInput: RefObject<HTMLInputElement | null>;
  expanded: boolean;
  close: () => void;
  create: () => void;
  account: ReactNode;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const workspaces =
    data?.workspaces.filter((value) =>
      `${value.title} ${value.goal}`
        .toLocaleLowerCase()
        .includes(filter.toLocaleLowerCase()),
    ) ?? [];
  return (
    <aside
      className={`intent-sidebar ${expanded ? "is-open" : ""}`}
      aria-label={ja ? "依頼の履歴" : "Request history"}
    >
      <div className="intent-brand">
        <Link
          to="/$section"
          params={{ section: "collaboration" }}
          search={{ channel: current || undefined }}
          onClick={close}
        >
          <img
            src={theme === "dark" ? logoOnDark : logo}
            width={136}
            height={51}
            alt="Aidash"
          />
        </Link>
        <Button
          variant="ghost"
          size="icon"
          className="intent-sidebar-close"
          aria-label={ja ? "履歴を閉じる" : "Close history"}
          onClick={close}
        >
          <PanelLeftClose />
        </Button>
      </div>
      <Button className="intent-new-request" onClick={create}>
        <Plus />
        {ja ? "新しい依頼" : "New request"}
      </Button>
      <label className="intent-history-search">
        <Search size={16} />
        <span className="sr-only">{ja ? "履歴を検索" : "Search history"}</span>
        <Input
          ref={searchInput}
          type="search"
          placeholder={ja ? "履歴を検索" : "Search history"}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <kbd>⌘ K</kbd>
      </label>
      {section !== "collaboration" && (
        <Link
          className="intent-back"
          to="/$section"
          params={{ section: "collaboration" }}
          search={{ channel: current || undefined }}
          onClick={close}
        >
          <ArrowLeft size={16} />
          {ja ? "依頼に戻る" : "Back to request"}
        </Link>
      )}
      <p className="intent-history-label">
        {ja ? "依頼の履歴" : "Recent requests"}
      </p>
      <nav
        className="intent-history"
        aria-label={ja ? "ワークスペース" : "Workspaces"}
      >
        {workspaces.map((value) => (
          <Link
            key={value.id}
            className={`collab-channel-link ${section === "collaboration" && current === value.id ? "selected" : ""}`}
            to="/$section"
            params={{ section: "collaboration" }}
            search={{ channel: value.id }}
            onClick={close}
            aria-label={`# ${value.title}`}
          >
            <MessageSquare size={15} />
            <span>{value.title}</span>
          </Link>
        ))}
        {data && workspaces.length === 0 && (
          <p className="muted">
            {ja
              ? filter
                ? "一致する依頼がありません。"
                : "最初の依頼を始めましょう。"
              : filter
                ? "No matching requests."
                : "Start your first request."}
          </p>
        )}
      </nav>
      <div className="intent-account">{account}</div>
    </aside>
  );
}
