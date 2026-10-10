import type { Task } from "../../types";
import { statusTone, useI18n, type StatusTone } from "../../ui";
import { cn } from "../../lib/utils";

/** Fill colour per semantic tone; used for status dots, segments and timeline ticks. */
export const toneFill: Record<StatusTone, string> = {
  brand: "bg-brand-mark",
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-destructive",
  neutral: "bg-edge",
};

/** One segment per task, coloured by status, in snapshot order. */
export function SegmentedProgress({
  tasks,
  label,
  className,
}: {
  tasks: readonly Task[];
  label: string;
  className?: string;
}) {
  const { t } = useI18n();
  const completed = tasks.filter((task) => task.status === "COMPLETED").length;
  return (
    <span
      role="img"
      aria-label={`${label} ${completed}/${tasks.length}`}
      className={cn("flex h-1.5 min-w-16 gap-0.5", className)}
    >
      {tasks.length === 0 && <span className="flex-1 rounded-sm bg-raised" />}
      {tasks.map((task) => (
        <span
          key={task.id}
          title={`${task.title} · ${t(task.status)}`}
          className={cn("flex-1 rounded-sm", toneFill[statusTone(task.status)])}
        />
      ))}
    </span>
  );
}

/** Status word with a tone dot; live phases pulse (motion-reduce safe). */
export function Phase({
  status,
  className,
}: {
  status: string;
  className?: string;
}) {
  const { t } = useI18n();
  const tone = statusTone(status);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 text-xs font-medium",
        {
          brand: "text-brand",
          success: "text-success",
          warning: "text-warning",
          danger: "text-destructive",
          neutral: "text-muted-foreground",
        }[tone],
        className,
      )}
    >
      <span
        aria-hidden
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          toneFill[tone],
          (status === "THINKING" || status === "TOOL_CALL") &&
            "animate-pulse motion-reduce:animate-none",
        )}
      />
      {t(status)}
    </span>
  );
}
