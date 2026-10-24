import { useState } from "react";
import { History } from "lucide-react";
import type { MeshEvent } from "../../types";
import { Button } from "../../components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "../../components/ui/tooltip";
import { useI18n } from "../../ui";
import { cn } from "../../lib/utils";

type Kind = "task" | "run" | "tool" | "request" | "artifact" | "peer" | "error";

const kindCopy: Record<"ja-JP" | "en-US", Record<Kind, string>> = {
  "ja-JP": {
    task: "タスク",
    run: "実行",
    tool: "ツール",
    request: "判断",
    artifact: "成果物",
    peer: "ピア",
    error: "エラー",
  },
  "en-US": {
    task: "Task",
    run: "Run",
    tool: "Tool",
    request: "Decision",
    artifact: "Artifact",
    peer: "Peer",
    error: "Error",
  },
};

const kindMark: Record<Kind, string> = {
  task: "rounded-[1px] bg-foreground",
  run: "rounded-full bg-brand-mark",
  tool: "h-2.5 w-[3px] rounded-[1px] bg-muted-foreground",
  request: "rotate-45 rounded-[1px] bg-warning",
  artifact: "rounded-[1px] border-[1.5px] border-success bg-transparent",
  peer: "rotate-45 rounded-[1px] bg-edge-strong",
  error: "rounded-[1px] bg-destructive",
};

/** Server event kinds are dotted words (`task.completed`, `run.phase`, `tool.invoke`). */
function eventKind(kind: string): Kind {
  if (/fail|error|invalid|abandon|cancel/.test(kind)) return "error";
  if (/human|approval|request/.test(kind)) return "request";
  if (kind.startsWith("tool.") || kind.includes("tool_")) return "tool";
  if (kind.startsWith("artifact.")) return "artifact";
  if (kind.startsWith("peer.") || kind.includes("remote")) return "peer";
  if (kind.startsWith("run.")) return "run";
  return "task";
}

/** Bottom strip: this request's events over the visible range with a current-time marker. */
export function TimelineStrip({
  events,
  now,
  history,
}: {
  events: MeshEvent[];
  now: number;
  history: () => void;
}) {
  const { locale } = useI18n();
  const labels = kindCopy[locale];
  const sorted = [...events].sort(
    (a, b) => Date.parse(a.created_at) - Date.parse(b.created_at),
  );
  const first = sorted.length ? Date.parse(sorted[0].created_at) : now;
  const last = sorted.length ? Date.parse(sorted.at(-1)!.created_at) : now;
  // Follow the present while the request is active; otherwise frame its last day of activity.
  const live = now - last <= 86_400_000;
  const end = live ? now : last + Math.max(60_000, (last - first) * 0.05);
  const start = Math.min(
    Math.max(first - (end - first) * 0.08, end - 86_400_000),
    end - 900_000,
  );
  const ordered = sorted.filter((event) => {
    const time = Date.parse(event.created_at);
    return time >= start && time <= end;
  });
  const [selected, setSelected] = useState<string | null>(null);
  const current =
    ordered.find((event) => event.id === selected) ?? ordered.at(-1);
  const at = (time: number) => `${((time - start) / (end - start)) * 100}%`;
  const clock = (time: number) =>
    new Date(time).toLocaleTimeString(locale, {
      hour: "2-digit",
      minute: "2-digit",
    });
  const axis = Array.from(
    { length: 6 },
    (_, index) => start + ((end - start) * index) / 5,
  );
  const title = locale === "ja-JP" ? "タイムライン" : "Timeline";
  return (
    <section
      aria-label={title}
      className="@container hidden h-[88px] shrink-0 flex-col border-t border-border bg-surface px-4 pt-2 md:flex"
    >
      <header className="flex h-6 items-center gap-3 text-[11px]">
        <span className="text-xs font-semibold text-foreground">{title}</span>
        <span className="whitespace-nowrap font-mono tabular text-faint">
          {clock(start)}–{clock(end)}
        </span>
        <span className="hidden items-center gap-3 text-faint @4xl:flex">
          {(Object.keys(labels) as Kind[]).map((kind) => (
            <span key={kind} className="inline-flex items-center gap-1.5">
              <span aria-hidden className={cn("size-2", kindMark[kind])} />
              {labels[kind]}
            </span>
          ))}
        </span>
        <span className="ml-auto flex min-w-0 items-center gap-2">
          {current && (
            <span className="hidden min-w-0 items-center gap-2 @xl:flex">
              <span className="font-mono tabular text-muted-foreground">
                {new Date(current.created_at).toLocaleTimeString(locale)}
              </span>
              <span className="truncate font-mono text-foreground">
                {current.kind}
              </span>
            </span>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-xs"
            type="button"
            onClick={history}
          >
            <History />
            {locale === "ja-JP" ? "実行履歴" : "Execution history"}
          </Button>
        </span>
      </header>
      <div className="flex min-h-0 flex-1 items-stretch gap-3">
        <span className="flex w-24 shrink-0 items-center gap-1 text-[11px] text-faint">
          <span className="font-mono tabular text-muted-foreground">
            {ordered.length}
          </span>
          {locale === "ja-JP" ? "件のイベント" : "events"}
        </span>
        <TooltipProvider delayDuration={150}>
          <div className="relative min-w-0 flex-1">
            {axis.map((time, index) => (
              <span
                key={time}
                className="absolute inset-y-0 top-1 bottom-4 border-l border-border"
                style={{ left: at(time) }}
              >
                <span
                  className={cn(
                    "absolute -bottom-4 font-mono text-[10px] tabular text-faint",
                    index === axis.length - 1
                      ? "right-0"
                      : index === 0
                        ? "left-0"
                        : "-translate-x-1/2",
                  )}
                >
                  {clock(time)}
                </span>
              </span>
            ))}
            {ordered.map((event) => {
              const kind = eventKind(event.kind);
              const time = Date.parse(event.created_at);
              const label = `${new Date(time).toLocaleString(locale)} ${labels[kind]} ${event.kind}`;
              return (
                <Tooltip key={event.id}>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      aria-label={label}
                      aria-pressed={current?.id === event.id}
                      onClick={() => setSelected(event.id)}
                      className="group absolute top-1 bottom-4 flex w-3 -translate-x-1/2 flex-col items-center justify-center gap-1 rounded-sm"
                      style={{ left: at(time) }}
                    >
                      <span
                        aria-hidden
                        className="h-3 w-px bg-border-strong group-aria-pressed:bg-foreground"
                      />
                      <span
                        aria-hidden
                        className={cn(
                          "size-2 group-hover:scale-125 motion-reduce:group-hover:scale-100",
                          kindMark[kind],
                        )}
                      />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>
                    <span className="font-mono tabular text-muted-foreground">
                      {new Date(time).toLocaleTimeString(locale)}
                    </span>
                    <span>{labels[kind]}</span>
                    <span className="font-mono">{event.kind}</span>
                  </TooltipContent>
                </Tooltip>
              );
            })}
            {live && (
              <span
                aria-hidden
                className="absolute top-0 bottom-4 w-0.5 -translate-x-1/2 rounded-full bg-brand-mark"
                style={{ left: at(now) }}
              >
                <span className="absolute -top-0.5 right-1 rounded-sm bg-brand-mark px-1 font-mono text-[10px] font-medium leading-4 text-primary-foreground">
                  {clock(now)}
                </span>
              </span>
            )}
          </div>
        </TooltipProvider>
      </div>
    </section>
  );
}
