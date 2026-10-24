import type { RefObject } from "react";
import { Link } from "@tanstack/react-router";
import { Plus, Search } from "lucide-react";
import { Button } from "../components/ui/button";
import { Kbd } from "../components/ui/kbd";
import { cn } from "../lib/utils";
import { taskProgress } from "../collaboration/model";
import type { State } from "../types";
import { useI18n } from "../ui";
import { shellCopy } from "./copy";

const dotTone = {
  attention: "bg-warning",
  active: "bg-brand-mark",
  done: "bg-success",
  idle: "bg-faint",
} as const;

/** Searchable request history: the workspace switcher body on desktop and in the mobile menu. */
export function WorkspaceList({
  data,
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
  const query = filter.toLocaleLowerCase();
  const workspaces =
    data?.workspaces.filter((value) =>
      `${value.title} ${value.goal}`.toLocaleLowerCase().includes(query),
    ) ?? [];
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
        aria-label={copy.workspaces}
        className="flex min-h-0 flex-col gap-px overflow-y-auto"
      >
        {workspaces.map((workspace) => {
          const progress = taskProgress(data?.tasks ?? [], workspace.id);
          const pending =
            data?.human_requests.filter(
              (request) =>
                request.workspace_id === workspace.id &&
                request.response === null,
            ).length ?? 0;
          const tone =
            progress.attention > 0
              ? "attention"
              : progress.active > 0
                ? "active"
                : progress.total > 0 && progress.completed === progress.total
                  ? "done"
                  : "idle";
          const selected = workspace.id === current;
          return (
            <Link
              key={workspace.id}
              to="/$section"
              params={{ section: "collaboration" }}
              search={{ channel: workspace.id }}
              onClick={navigate}
              aria-label={`# ${workspace.title}`}
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
                    {workspace.title}
                  </span>
                  {pending > 0 && (
                    <span className="shrink-0 rounded-sm bg-warning-soft px-1.5 font-mono text-[11px] leading-[18px] text-warning tabular">
                      {copy.awaiting(pending)}
                    </span>
                  )}
                </span>
                <span className="mt-0.5 line-clamp-1 block text-xs text-muted-foreground">
                  {workspace.goal}
                </span>
              </span>
              {progress.total > 0 && (
                <span className="font-mono text-xs text-muted-foreground tabular">
                  {progress.completed}/{progress.total}
                </span>
              )}
            </Link>
          );
        })}
        {data && workspaces.length === 0 && (
          <p className="px-2 py-3 text-xs text-muted-foreground">
            {filter ? copy.noMatch : copy.startFirst}
          </p>
        )}
      </nav>
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
