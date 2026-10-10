import { useId, type ComponentProps, type ReactNode } from "react";
import {
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Info,
  TriangleAlert,
} from "lucide-react";
import { Badge as ToneBadge } from "./ui/badge";
import { Button } from "./ui/button";
import { Skeleton } from "./ui/skeleton";
import { controlClass } from "./ui/control";
import { cn } from "../lib/utils";
import { statusTone, useI18n, type StatusTone } from "../ui";

/*
 * Shared Night Ops presentation patterns. Screens compose these instead of
 * defining their own hint/notice/error/disclosure/metric variants.
 */

/** Vertical rhythm shared by dialog and settings forms. */
export const formClass = "grid min-w-0 gap-4";

/** Two equal columns that stack on narrow screens. */
export const pairClass = "grid min-w-0 gap-4 sm:grid-cols-2";

/** Controls on one wrapping row, aligned on their bottom edge; full width on phones. */
export const inlineFormClass =
  "flex min-w-0 flex-wrap items-end gap-2 max-sm:[&>*]:w-full";

/** Native multi-select list box styled like other controls. */
export const multiSelectClass = cn(
  controlClass,
  "min-h-24 py-1 [&>option]:rounded-sm [&>option]:px-1.5 [&>option]:py-0.5",
);

/** Secondary explanatory copy under a field, section or list. Pass `role="status"` when it reflects live state. */
export function Hint({ className, ...props }: ComponentProps<"p">) {
  return (
    <p
      className={cn(
        "max-w-[72ch] text-xs leading-relaxed text-muted-foreground [overflow-wrap:anywhere]",
        className,
      )}
      {...props}
    />
  );
}

const noticeTone = {
  info: "border-border bg-raised text-muted-foreground",
  warning: "border-warning/30 bg-warning-soft text-warning",
} as const;

/** Inline informational or cautionary message. */
export function Notice({
  tone = "info",
  className,
  children,
  ...props
}: ComponentProps<"div"> & { tone?: keyof typeof noticeTone }) {
  const Icon = tone === "warning" ? TriangleAlert : Info;
  return (
    <div
      className={cn(
        "flex min-w-0 items-start gap-2 rounded-md border px-3 py-2 text-xs leading-relaxed",
        noticeTone[tone],
        className,
      )}
      {...props}
    >
      <Icon aria-hidden className="mt-0.5 size-3.5 shrink-0" />
      <div className="min-w-0 flex-1 [overflow-wrap:anywhere]">{children}</div>
    </div>
  );
}

/** Inline error box; the message is the `role="alert"` region and the optional retry action sits beside it. */
export function Alert({
  retry,
  retryLabel,
  className,
  children,
  ...props
}: Omit<ComponentProps<"div">, "role"> & {
  retry?: () => void;
  /** Defaults to the shared "retry" copy. */
  retryLabel?: string;
}) {
  const { t } = useI18n();
  return (
    <div
      className={cn(
        "flex min-w-0 items-start gap-2 rounded-md border border-destructive/30 bg-destructive-soft px-3 py-2 text-xs leading-relaxed text-destructive",
        className,
      )}
      {...props}
    >
      <div role="alert" className="flex min-w-0 flex-1 items-start gap-2">
        <CircleAlert aria-hidden className="mt-0.5 size-3.5 shrink-0" />
        <div className="min-w-0 flex-1 [overflow-wrap:anywhere]">
          {children}
        </div>
      </div>
      {retry && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="-my-1 shrink-0"
          onClick={retry}
        >
          {retryLabel ?? t("retry")}
        </Button>
      )}
    </div>
  );
}

/** Live loading line (`role="status"`); defaults to the shared loading copy. */
export function Loading({
  children,
  className,
}: {
  children?: ReactNode;
  className?: string;
}) {
  const { t } = useI18n();
  return (
    <p role="status" className={cn("py-2 text-xs text-faint", className)}>
      {children ?? t("loading")}
    </p>
  );
}

/** Loading shape for a form screen. */
export function FormSkeleton({ label }: { label: string }) {
  return (
    <div
      role="status"
      aria-label={label}
      className="grid max-w-[760px] gap-5 px-4 py-6 md:px-6"
    >
      <Skeleton className="h-4 w-40" />
      {[0, 1, 2, 3].map((row) => (
        <div key={row} className="grid gap-2">
          <Skeleton className="h-3 w-24" />
          <Skeleton className="h-8 w-full" />
        </div>
      ))}
      <Skeleton className="h-20 w-full" />
    </div>
  );
}

/** Loading shape for a metrics + lists screen. */
export function OverviewSkeleton({ label }: { label: string }) {
  return (
    <div role="status" aria-label={label} className="grid gap-6">
      <div className="grid grid-cols-2 border-b border-border sm:grid-cols-3 xl:grid-cols-5">
        {[0, 1, 2, 3, 4].map((cell) => (
          <div key={cell} className="grid gap-2 px-5 py-4">
            <Skeleton className="h-3 w-20" />
            <Skeleton className="h-7 w-16" />
          </div>
        ))}
      </div>
      <div className="grid gap-3 px-4 md:px-6">
        {[0, 1, 2].map((row) => (
          <Skeleton key={row} className="h-9 w-full" />
        ))}
      </div>
    </div>
  );
}

/** Checkbox with its label text; the label wraps the control so the accessible name is the text. */
export function Check({
  children,
  className,
  ...input
}: Omit<ComponentProps<"input">, "type" | "children"> & {
  children: ReactNode;
}) {
  return (
    <label
      className={cn(
        "flex min-w-0 cursor-pointer items-start gap-2 text-[13px] text-foreground has-[:disabled]:cursor-not-allowed has-[:disabled]:text-muted-foreground",
        className,
      )}
    >
      <input
        type="checkbox"
        className="mt-[3px] size-3.5 shrink-0 cursor-pointer accent-primary disabled:cursor-not-allowed"
        {...input}
      />
      <span className="min-w-0 [overflow-wrap:anywhere]">{children}</span>
    </label>
  );
}

/** Fieldset grouped by a hairline (section) or a left rule (nested item). */
export function Group({
  legend,
  nested = false,
  className,
  children,
  ...props
}: Omit<ComponentProps<"fieldset">, "children"> & {
  legend: ReactNode;
  nested?: boolean;
  children: ReactNode;
}) {
  return (
    <fieldset
      className={cn(
        "grid min-w-0 gap-3",
        nested
          ? "border-l-2 border-border pl-3"
          : "border-t border-border pt-4 first:border-t-0 first:pt-0 [input[type=hidden]:first-child+&]:border-t-0 [input[type=hidden]:first-child+&]:pt-0",
        className,
      )}
      {...props}
    >
      <legend
        className={cn(
          "float-left w-full",
          nested
            ? "text-xs font-medium text-muted-foreground"
            : "text-[13px] font-semibold text-foreground",
        )}
      >
        {legend}
      </legend>
      {children}
    </fieldset>
  );
}

/** Collapsible block on native details/summary: quiet summary row, content tied to it by a hairline. */
export function Disclosure({
  summary,
  className,
  children,
  ...props
}: Omit<ComponentProps<"details">, "children"> & {
  summary: ReactNode;
  children: ReactNode;
}) {
  return (
    <details className={cn("group/disclosure min-w-0", className)} {...props}>
      <summary className="flex min-h-7 w-fit max-w-full cursor-pointer list-none items-center gap-1.5 py-1 text-xs font-medium text-muted-foreground transition-colors duration-150 select-none hover:text-foreground [&::-webkit-details-marker]:hidden">
        <ChevronRight
          aria-hidden
          className="size-3.5 shrink-0 transition-transform duration-150 group-open/disclosure:rotate-90 motion-reduce:transition-none"
        />
        <span className="min-w-0 [overflow-wrap:anywhere]">{summary}</span>
      </summary>
      <div className="ml-[7px] mt-1 grid min-w-0 gap-3 border-l border-border pb-1 pl-4 pt-1">
        {children}
      </div>
    </details>
  );
}

/** Monospace preformatted text block. */
export function Pre({ className, ...props }: ComponentProps<"pre">) {
  return (
    <pre
      className={cn(
        "max-h-52 overflow-auto whitespace-pre-wrap rounded-md bg-raised p-2.5 font-mono text-xs leading-relaxed text-muted-foreground [overflow-wrap:anywhere]",
        className,
      )}
      {...props}
    />
  );
}

export type Fact = readonly [
  label: ReactNode,
  value: ReactNode,
  mono?: boolean,
];

/** Label/value pairs: two aligned columns, stacked on phones. */
export function Facts({
  items,
  className,
}: {
  items: readonly Fact[];
  className?: string;
}) {
  return (
    <dl
      className={cn(
        "grid min-w-0 grid-cols-1 gap-y-0.5 text-[13px] leading-normal sm:grid-cols-[minmax(6rem,max-content)_minmax(0,1fr)] sm:gap-x-4 sm:gap-y-1.5",
        className,
      )}
    >
      {items.map(([label, value, mono], index) => (
        <div key={index} className="contents">
          <dt className="text-xs leading-5 text-faint">{label}</dt>
          <dd
            className={cn(
              "mb-1.5 min-w-0 text-foreground [overflow-wrap:anywhere] sm:mb-0",
              mono && "font-mono text-xs leading-5 tabular",
            )}
          >
            {value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

const metricColumns = {
  2: "grid-cols-2",
  4: "grid-cols-2 sm:grid-cols-4",
  5: "grid-cols-2 sm:grid-cols-3 xl:grid-cols-5",
  6: "grid-cols-3 sm:grid-cols-6",
} as const;

/**
 * Row of metrics (a `dl`) separated by hairlines, no boxed cards; works on any
 * surface. `compact` scales numerals down for list rows, dialogs and rails.
 */
export function MetricRow({
  label,
  columns = 5,
  compact = false,
  className,
  children,
}: {
  /** Accessible group name. */
  label?: string;
  columns?: keyof typeof metricColumns;
  compact?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      role={label ? "group" : undefined}
      aria-label={label}
      data-compact={compact || undefined}
      className={cn(
        "group/metrics min-w-0 overflow-hidden border-b border-border",
        className,
      )}
    >
      <dl
        className={cn(
          "-mb-px -mr-px grid [&>*]:border-b [&>*]:border-r [&>*]:border-border",
          metricColumns[columns],
        )}
      >
        {children}
      </dl>
    </div>
  );
}

/** Factual metric: label, mono numeral (or short word), optional caption. */
export function Metric({
  label,
  value,
  caption,
  tone,
  className,
}: {
  label: string;
  value: ReactNode;
  caption?: string;
  tone?: StatusTone;
  /** Grid placement (e.g. spanning the last cell of an odd row). */
  className?: string;
}) {
  const shown = String(value);
  const count = typeof value === "number" || /^(\d+ \/ \d+|—)$/.test(shown);
  return (
    <div
      className={cn(
        "flex min-w-0 flex-col gap-1.5 px-4 py-3 md:px-5 md:py-4 group-data-[compact]/metrics:gap-0.5 group-data-[compact]/metrics:px-3 group-data-[compact]/metrics:py-2",
        className,
      )}
    >
      <dt className="flex min-w-0 items-center gap-1.5 truncate text-[11px] text-faint">
        {tone && <Dot tone={tone} />}
        {label}
      </dt>
      <dd
        className={cn(
          "min-w-0 truncate leading-8 text-foreground group-data-[compact]/metrics:text-[13px] group-data-[compact]/metrics:leading-5",
          count
            ? "font-mono text-[28px] font-medium tabular"
            : /\d/.test(shown)
              ? "font-mono text-[15px] font-medium tabular"
              : "text-[15px] font-semibold",
        )}
      >
        {value}
      </dd>
      {caption && <dd className="text-[11px] text-faint">{caption}</dd>}
    </div>
  );
}

/** Status dot fill per semantic tone. */
export const dotTone: Record<StatusTone, string> = {
  brand: "bg-brand-mark",
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-destructive",
  neutral: "bg-faint",
};

export function Dot({
  tone,
  className,
}: {
  tone: StatusTone;
  className?: string;
}) {
  return (
    <span
      aria-hidden
      className={cn("size-1.5 shrink-0 rounded-full", dotTone[tone], className)}
    />
  );
}

/** Tone badge with a status dot, for already-localised labels (same look as `ui.tsx` Badge). */
export function StateBadge({
  tone,
  className,
  children,
}: {
  tone: StatusTone;
  className?: string;
  children: ReactNode;
}) {
  return (
    <ToneBadge tone={tone} className={className}>
      <span aria-hidden className="size-1.5 rounded-full bg-current" />
      {children}
    </ToneBadge>
  );
}

/** Raw server status word with its semantic tone (no translation). */
export function StatusWord({ value }: { value: string }) {
  return <StateBadge tone={statusTone(value)}>{value}</StateBadge>;
}

/** Saved / unsaved indicator for a screen header. */
export function SaveState({ dirty, label }: { dirty: boolean; label: string }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
      <Dot tone={dirty ? "warning" : "success"} />
      {label}
    </span>
  );
}

/** Empty / unavailable state with a heading. */
export function EmptyState({
  icon,
  title,
  children,
  action,
  className,
}: {
  icon?: ReactNode;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex flex-col items-start gap-1.5 rounded-lg border border-dashed border-border-strong px-4 py-5",
        className,
      )}
    >
      {icon && <span className="mb-1 text-faint [&_svg]:size-5">{icon}</span>}
      <h3 className="text-[13px] font-medium text-foreground">{title}</h3>
      {children && (
        <p className="max-w-[60ch] text-xs text-muted-foreground">{children}</p>
      )}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

/** Dense hairline list container. */
export function RowList({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "min-w-0 divide-y divide-border border-y border-border",
        className,
      )}
    >
      {children}
    </div>
  );
}

type PageStep = {
  disabled: boolean;
  go: () => void;
  /** Defaults to the shared previous/next page copy. */
  label?: string;
};

/** Previous/next pager for cursor or offset paging. */
export function Pager({
  page,
  previous,
  next,
  className,
}: {
  page: number;
  previous: PageStep;
  next: PageStep;
  className?: string;
}) {
  const { t } = useI18n();
  return (
    <div className={cn("flex items-center justify-end gap-2 pt-1", className)}>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={previous.disabled}
        onClick={previous.go}
      >
        <ChevronLeft aria-hidden />
        {previous.label ?? t("authPrevious")}
      </Button>
      <span className="text-xs text-muted-foreground">
        {t("authPage")} <span className="font-mono tabular">{page}</span>
      </span>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={next.disabled}
        onClick={next.go}
      >
        {next.label ?? t("authNext")}
        <ChevronRight aria-hidden />
      </Button>
    </div>
  );
}

/** Expandable audit/history row: summary line with a mono timestamp, record below. */
export function HistoryRow({
  summary,
  time,
  className,
  children,
}: {
  summary: ReactNode;
  time: string;
  /** Test-hook classes (e.g. `auth-history`). */
  className?: string;
  children: ReactNode;
}) {
  return (
    <details className={cn("group/row min-w-0", className)}>
      <summary className="grid cursor-pointer list-none grid-cols-[auto_minmax(0,1fr)] items-center gap-x-2 gap-y-0.5 px-1 py-2 text-xs transition-colors duration-150 hover:bg-accent sm:grid-cols-[auto_minmax(0,1fr)_auto] [&::-webkit-details-marker]:hidden">
        <ChevronRight
          aria-hidden
          className="size-3.5 text-faint transition-transform duration-150 group-open/row:rotate-90 motion-reduce:transition-none"
        />
        <span className="min-w-0 truncate text-foreground">{summary}</span>
        <time className="col-start-2 truncate font-mono text-[11px] text-faint sm:col-start-auto">
          {time}
        </time>
      </summary>
      <div className="min-w-0 pb-3 pl-6">{children}</div>
    </details>
  );
}

/**
 * Management screen header row: title (optionally `/ section`), mono meta,
 * status, quiet description and actions. `wb-actions` is a spec hook.
 */
export function ScreenHeader({
  title,
  section,
  meta,
  status,
  description,
  actions,
}: {
  title: ReactNode;
  /** Second breadcrumb level, rendered as a muted h2 after a slash. */
  section?: ReactNode;
  meta?: ReactNode;
  status?: ReactNode;
  /** Prose help shown on wide screens only. */
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b border-border px-4 py-2 md:px-6">
      <div className="flex min-w-0 flex-1 basis-64 flex-col gap-0.5 sm:flex-row sm:items-baseline sm:gap-3">
        <div className="flex min-w-0 items-baseline gap-2">
          <h1 className="truncate text-[15px] font-semibold tracking-tight text-foreground">
            {title}
          </h1>
          {section && (
            <>
              <span aria-hidden className="text-faint">
                /
              </span>
              <h2 className="min-w-0 truncate text-[15px] font-medium text-muted-foreground">
                {section}
              </h2>
            </>
          )}
        </div>
        {meta && (
          <p className="truncate font-mono text-xs text-faint">{meta}</p>
        )}
      </div>
      {description && (
        <p className="hidden min-w-0 truncate text-xs text-faint lg:block">
          {description}
        </p>
      )}
      {status}
      {actions && (
        <div className="wb-actions flex flex-wrap items-center gap-2">
          {actions}
        </div>
      )}
    </header>
  );
}

/**
 * Hairline-separated section of a form or main column; the parent grid's gap
 * sets the spacing. Not a named region: section titles often repeat a field
 * label and must not compete with it.
 */
export function Section({
  title,
  description,
  action,
  level = 2,
  className,
  children,
}: {
  title: string;
  description?: ReactNode;
  action?: ReactNode;
  /** Heading level; use 3 inside dialogs whose title is the h2. */
  level?: 2 | 3;
  className?: string;
  children: ReactNode;
}) {
  const Heading = level === 3 ? "h3" : "h2";
  return (
    <section
      className={cn(
        "grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4 border-t border-border pt-5 first:border-t-0 first:pt-0",
        className,
      )}
    >
      <div className="flex min-h-7 flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <div className="grid min-w-0 gap-1">
          <Heading className="text-[13px] font-semibold text-foreground">
            {title}
          </Heading>
          {description && (
            <p className="max-w-[72ch] text-xs leading-relaxed text-muted-foreground">
              {description}
            </p>
          )}
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}

/** Two-column body (main + inspector); add test-hook classes alongside. */
export const bodyClass =
  "min-w-0 lg:grid lg:min-h-0 lg:flex-1 lg:grid-cols-[minmax(0,1fr)_360px] lg:overflow-hidden";

/** Scrolling main column with a readable max width; children are spaced by hairline sections. */
export function MainColumn({
  children,
  wide = false,
}: {
  children: ReactNode;
  wide?: boolean;
}) {
  return (
    <div className="min-w-0 px-4 py-5 md:px-6 lg:min-h-0 lg:overflow-y-auto">
      <div
        className={cn(
          "grid min-w-0 grid-cols-[minmax(0,1fr)] gap-6",
          wide ? "max-w-6xl" : "max-w-[760px]",
        )}
      >
        {children}
      </div>
    </div>
  );
}

/** Right inspector column. Stacks under the main column below `lg`. */
export function Inspector({
  label,
  className,
  children,
}: {
  label: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <aside
      aria-label={label}
      className={cn(
        "min-w-0 border-t border-border bg-surface lg:min-h-0 lg:overflow-y-auto lg:border-t-0 lg:border-l",
        className,
      )}
    >
      {children}
    </aside>
  );
}

/** Named section of an inspector rail. */
export function InspectorSection({
  title,
  count,
  action,
  level = 2,
  className,
  children,
}: {
  title: string;
  count?: number;
  action?: ReactNode;
  /** Heading level; use 3 when the inspector itself is titled by an h2. */
  level?: 2 | 3;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  const Heading = level === 3 ? "h3" : "h2";
  return (
    <section
      aria-labelledby={id}
      className={cn(
        "grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2.5 border-b border-border px-4 py-4 last:border-b-0",
        className,
      )}
    >
      <div className="flex min-h-6 items-center justify-between gap-2">
        <Heading
          id={id}
          className="flex min-w-0 items-center gap-2 text-xs font-semibold text-foreground"
        >
          {title}
          {count !== undefined && (
            <span className="font-mono font-normal text-faint tabular">
              {count}
            </span>
          )}
        </Heading>
        {action}
      </div>
      {children}
    </section>
  );
}
