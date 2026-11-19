import type { RefObject } from "react";
import { Link } from "@tanstack/react-router";
import { Plus, Search } from "lucide-react";
import { Button } from "../components/ui/button";
import { Kbd } from "../components/ui/kbd";
import { cn } from "../lib/utils";
import type { State } from "../types";
import { useI18n } from "../ui";
import { shellCopy } from "./copy";
import { threadStatus } from "./thread-status";

const dotTone = {
  attention: "bg-warning",
  active: "bg-brand-mark",
  done: "bg-success",
  idle: "bg-faint",
  // Older Tasks lie beyond the window, so neither done nor idle is known.
  unknown: "border border-faint bg-transparent",
} as const;

/** Searchable request history: the thread switcher body on desktop and in the mobile menu. */
export function ThreadList({
  data,
  more,
  current,
  filter,
  setFilter,
  searchInput,
  create,
  showCreate,
  navigate,
  className,
}: {
  data?: State;
  /** "Show more" beyond the Thread window; the pages live with State. */
  more: {
    available: boolean;
    loading: boolean;
    error: boolean;
    load: () => void;
  };
  current: string;
  filter: string;
  setFilter: (value: string) => void;
  searchInput: RefObject<HTMLInputElement | null>;
  create: () => void;
  showCreate: boolean;
  navigate: () => void;
  className?: string;
}) {
  const { locale } = useI18n();
  const copy = shellCopy[locale];
  const windows = {
    moreTasks: Boolean(data?.next_cursors.tasks),
    moreRequests: Boolean(data?.next_cursors.human_requests),
  };
  const query = filter.toLocaleLowerCase();
  const threads = (data?.threads ?? []).filter((value) =>
    value.title.toLocaleLowerCase().includes(query),
  );
  return (
    <div className={cn("flex min-h-0 flex-col", className)}>
      <label className="relative mx-1 mb-1 flex items-center">
        <Search
          aria-hidden
          className="pointer-events-none absolute left-2.5 size-3.5 text-faint"
        />
        <span className="sr-only">{copy.searchHistory}</span>
        <input
          ref={searchInput}
          type="search"
          placeholder={copy.searchHistory}
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          className="h-8 w-full min-w-0 rounded-md border border-input bg-surface pl-8 pr-12 text-[13px] text-foreground transition-colors placeholder:text-faint hover:border-edge focus-visible:border-brand-line focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-brand-line [&::-webkit-search-cancel-button]:hidden"
        />
        <Kbd className="pointer-events-none absolute right-2">⌘K</Kbd>
      </label>
      <p className="px-2 pb-1 pt-2 text-[11px] text-faint">
        {copy.recentRequests}
      </p>
      <nav
        aria-label={copy.threads}
        className="flex min-h-0 flex-col gap-px overflow-y-auto"
      >
        {threads.map((thread) => {
          const { tone, progress, pending, pendingMore } = threadStatus(
            data?.tasks ?? [],
            data?.human_requests ?? [],
            thread.id,
            windows,
          );
          const selected = thread.id === current;
          return (
            <Link
              key={thread.id}
              to="/$section"
              params={{ section: "collaboration" }}
              search={{ channel: thread.id }}
              onClick={navigate}
              aria-label={`# ${thread.title}`}
              aria-current={selected ? "page" : undefined}
              className={cn(
                "relative grid grid-cols-[8px_minmax(0,1fr)_auto] items-start gap-2.5 rounded-md px-2 py-2 text-left transition-colors duration-150 hover:bg-accent",
                selected &&
                  "bg-brand-soft before:absolute before:inset-y-2 before:left-0 before:w-0.5 before:rounded-r-sm before:bg-brand-mark hover:bg-brand-soft",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "mt-1.5 size-1.5 rounded-full",
                  dotTone[tone],
                  tone === "active" &&
                    "animate-pulse motion-reduce:animate-none",
                )}
              />
              <span className="min-w-0">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="truncate font-mono text-[12.5px] font-medium text-foreground">
                    {thread.title}
                  </span>
                  {pending > 0 && (
                    <span className="shrink-0 rounded-sm bg-warning-soft px-1.5 font-mono text-[11px] leading-[18px] text-warning tabular">
                      {copy.awaiting(pending, pendingMore)}
                    </span>
                  )}
                </span>
              </span>
              {progress && (
                <span className="font-mono text-xs text-muted-foreground tabular">
                  {progress.completed}/{progress.total}
                </span>
              )}
            </Link>
          );
        })}
        {data && threads.length === 0 && (
          <p className="px-2 py-3 text-xs text-muted-foreground">
            {filter ? copy.noMatch : copy.startFirst}
          </p>
        )}
      </nav>
      {more.available && (
        <Button variant="ghost" onClick={more.load} disabled={more.loading}>
          {more.loading ? copy.loading : copy.showMore}
        </Button>
      )}
      {more.error && <p role="alert">{copy.moreError}</p>}
      {showCreate && (
        <div className="mt-1 border-t border-border pt-1">
          <Button
            variant="ghost"
            className="w-full justify-start text-foreground"
            onClick={create}
          >
            <Plus />
            {copy.newRequest}
          </Button>
        </div>
      )}
    </div>
  );
}
