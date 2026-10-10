import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import {
  Bell,
  CheckCircle2,
  CircleHelp,
  ShieldCheck,
  TriangleAlert,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { Toaster } from "./components/ui/sonner";
import { Button } from "./components/ui/button";
import { cn } from "./lib/utils";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "./components/ui/sheet";
import type { Mesh, State } from "./types";
import type { Locale } from "./ui";
import { Hint } from "./components/patterns";
import {
  NoticeStore,
  type Notice,
  type NoticeEntry,
  type NoticeFrame,
  type NoticeTarget,
} from "./notifications/model";

const copy = {
  "ja-JP": {
    notifications: "通知",
    description: "確認待ちの依頼と最近50件の重要な更新",
    pending: "確認待ち",
    recent: "最近の更新",
    empty: "通知はありません。",
    approval: "承認が必要です",
    input: "回答が必要です",
    completed: "タスクが完了しました",
    failed: "タスクが失敗しました",
    updates: (count: number) => `${count}件の更新があります`,
    view: "通知を確認",
    close: "通知を閉じる",
  },
  "en-US": {
    notifications: "Notifications",
    description: "Pending requests and the 50 most recent important updates",
    pending: "Awaiting your input",
    recent: "Recent updates",
    empty: "No notifications.",
    approval: "Approval required",
    input: "Your response is needed",
    completed: "Task completed",
    failed: "Task failed",
    updates: (count: number) => `${count} updates`,
    view: "View notifications",
    close: "Dismiss notification",
  },
};
const icons = {
  approval: ShieldCheck,
  input: CircleHelp,
  completed: CheckCircle2,
  failed: TriangleAlert,
};
const iconTone = {
  approval: "text-warning",
  input: "text-warning",
  completed: "text-success",
  failed: "text-destructive",
};
function frame(
  data: State | undefined,
  remote: Mesh | undefined,
): NoticeFrame | null {
  if (!data) return null;
  const scopes = data.workspaces.map((workspace) =>
    JSON.stringify([data.node.id, workspace.id]),
  );
  scopes.push(
    ...data.human_requests.map((request) =>
      JSON.stringify([data.node.id, request.workspace_id]),
    ),
  );
  const entries: NoticeEntry[] = data.tasks.map((task) => ({
    key: JSON.stringify([data.node.id, "task", task.id]),
    scope: JSON.stringify([data.node.id, task.workspace_id]),
    signature: `${task.revision}:${task.status}`,
    kind:
      task.status === "FAILED"
        ? "failed"
        : task.status === "COMPLETED"
          ? "completed"
          : null,
    detail: task.title.slice(0, 180),
    workspaceTitle:
      data.workspaces.find((workspace) => workspace.id === task.workspace_id)
        ?.title ?? "",
    target: {
      kind: "task",
      id: task.id,
      node: data.node.id,
      workspace: task.workspace_id,
    },
  }));
  const requests = [
    ...data.human_requests.map((request) => ({ request, node: data.node.id })),
    ...(remote?.nodes.flatMap((node) => {
      scopes.push(
        ...node.runs.map((run) =>
          JSON.stringify([node.node_id, run.workspace_id]),
        ),
      );
      scopes.push(
        ...node.human_requests.map((request) =>
          JSON.stringify([node.node_id, request.workspace_id]),
        ),
      );
      return node.human_requests.map((request) => ({
        request,
        node: node.node_id,
      }));
    }) ?? []),
  ];
  for (const { request, node } of requests)
    entries.push({
      key: JSON.stringify([node, "human", request.id]),
      scope: JSON.stringify([node, request.workspace_id]),
      signature: request.kind,
      kind:
        request.response !== null
          ? null
          : request.kind === "APPROVAL_REQUIRED"
            ? "approval"
            : "input",
      detail: request.prompt.slice(0, 180),
      workspaceTitle:
        node === data.node.id
          ? (data.workspaces.find(
              (workspace) => workspace.id === request.workspace_id,
            )?.title ?? node)
          : node,
      target: {
        kind: "human",
        id: request.id,
        node,
        workspace: request.workspace_id,
      },
    });
  return { scopes, entries };
}
const NotificationContext = createContext<{
  store: NoticeStore;
  show: () => void;
  label: string;
} | null>(null);
export function NotificationBell({ className }: { className?: string }) {
  const value = useContext(NotificationContext)!;
  const view = useSyncExternalStore(
    value.store.subscribe,
    value.store.getSnapshot,
  );
  return (
    <Button
      variant="ghost"
      size="icon"
      type="button"
      aria-label={value.label}
      onClick={value.show}
      className={cn("relative", className)}
    >
      <Bell aria-hidden strokeWidth={1.75} />
      {view.pending.length > 0 ? (
        <span className="absolute -right-0.5 -top-0.5 grid h-[15px] min-w-[15px] place-items-center rounded-sm bg-primary px-[3px] font-mono text-[10px] font-semibold leading-none text-primary-foreground ring-2 ring-rail tabular">
          {view.pending.length}
        </span>
      ) : (
        view.unread && (
          <span className="absolute right-1 top-1 size-1.5 rounded-full bg-brand-mark ring-2 ring-rail" />
        )
      )}
    </Button>
  );
}

export function NotificationProvider({
  data,
  remote,
  locale,
  theme,
  onSelect,
  children,
}: {
  data: State | undefined;
  remote: Mesh | undefined;
  locale: Locale;
  theme: "light" | "dark";
  onSelect: (target: NoticeTarget) => void;
  children: ReactNode;
}) {
  const words = copy[locale];
  const [store] = useState(() => new NoticeStore());
  const [centerOpen, setCenterOpen] = useState(false);
  const view = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const active = useRef<string[]>([]);
  const select = useRef(onSelect);
  const labels = useRef(words);
  const toastId = useId();
  useEffect(() => {
    select.current = onSelect;
    labels.current = words;
  }, [onSelect, words]);
  const show = useCallback(() => {
    store.markRead();
    setCenterOpen(true);
    toast.dismiss(toastId);
  }, [store, toastId]);
  const visit = useCallback(
    (item: Notice) => {
      const current = store.getCurrent(item.id);
      if (!current) return;
      store.markRead(current.id);
      setCenterOpen(false);
      toast.dismiss(toastId);
      select.current(current.target);
    },
    [store, toastId],
  );
  useEffect(() => {
    store.observe(frame(data, remote));
    if (centerOpen) store.markRead();
    if (
      active.current.length &&
      !active.current.some((id) => store.getCurrent(id))
    ) {
      toast.dismiss(toastId);
      active.current = [];
    }
    const delay = store.delay(Date.now());
    if (delay === undefined || timer.current !== undefined) return;
    timer.current = setTimeout(() => {
      timer.current = undefined;
      const batch = store.takeBatch(Date.now());
      if (!batch.length) return;
      active.current = batch.map((item) => item.id);
      const item = batch.length === 1 ? batch[0] : undefined;
      const Icon = item ? icons[item.kind] : Bell;
      const currentWords = labels.current;
      toast.custom(
        () => (
          <div className="flex w-[min(380px,calc(100vw-24px))] items-start gap-1 rounded-lg border border-border-strong bg-popover p-1.5 text-popover-foreground shadow-overlay">
            <Button
              variant="ghost"
              className="intent-toast-open h-auto min-w-0 flex-1 items-start justify-start gap-3 whitespace-normal px-2.5 py-2 text-left text-foreground [&_svg]:size-4"
              onClick={() => {
                if (item) visit(item);
                else show();
              }}
            >
              <Icon
                aria-hidden
                className={cn(
                  "mt-0.5",
                  item ? iconTone[item.kind] : "text-brand",
                )}
              />
              <span className="grid min-w-0 gap-0.5">
                <strong className="text-[13px] font-semibold">
                  {item
                    ? currentWords[item.kind]
                    : currentWords.updates(batch.length)}
                </strong>
                <span className="line-clamp-2 text-xs font-normal text-muted-foreground">
                  {item ? item.detail : currentWords.view}
                </span>
                {item && (
                  <small className="truncate font-mono text-[11px] font-normal text-faint">
                    {item.workspaceTitle}
                  </small>
                )}
              </span>
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="size-7 shrink-0"
              aria-label={currentWords.close}
              onClick={() => toast.dismiss(toastId)}
            >
              <X aria-hidden />
            </Button>
          </div>
        ),
        {
          id: toastId,
          toasterId: toastId,
          duration: 8000,
          unstyled: true,
          className: "intent-toast",
        },
      );
    }, delay);
  }, [data, remote, store, toastId, show, visit, centerOpen]);
  useEffect(
    () => () => {
      clearTimeout(timer.current);
      toast.dismiss(toastId);
    },
    [toastId],
  );
  const recent = view.recent.filter((item) => item.target.kind === "task");
  return (
    <NotificationContext value={{ store, show, label: words.notifications }}>
      {children}
      <Toaster
        id={toastId}
        theme={theme}
        visibleToasts={1}
        position="bottom-right"
        containerAriaLabel={words.notifications}
        offset={20}
        mobileOffset={12}
      />
      <Sheet
        open={centerOpen}
        onOpenChange={(open) => {
          if (open) store.markRead();
          setCenterOpen(open);
        }}
      >
        <SheetContent
          className="intent-notifications w-[min(420px,100vw)] gap-0 overflow-y-auto p-0"
          closeLabel={words.close}
        >
          <SheetHeader className="border-b border-border px-5 py-4">
            <SheetTitle>{words.notifications}</SheetTitle>
            <SheetDescription className="text-xs">
              {words.description}
            </SheetDescription>
          </SheetHeader>
          {view.pending.length === 0 && recent.length === 0 && (
            <Hint className="px-5 py-6">{words.empty}</Hint>
          )}
          {[
            { title: words.pending, items: view.pending },
            { title: words.recent, items: recent },
          ].map(
            (group) =>
              group.items.length > 0 && (
                <section
                  key={group.title}
                  className="grid gap-px border-b border-border px-3 py-3 last:border-b-0"
                >
                  <h3 className="flex items-center justify-between px-2 pb-1.5 text-[11px] font-medium text-faint">
                    {group.title}
                    <span className="font-mono tabular">
                      {group.items.length}
                    </span>
                  </h3>
                  {group.items.map((item) => {
                    const Icon = icons[item.kind];
                    return (
                      <Button
                        variant="ghost"
                        className="intent-notification-row h-auto w-full items-start justify-start gap-3 whitespace-normal px-2 py-2 text-left text-foreground [&_svg]:size-4"
                        key={item.id}
                        onClick={() => visit(item)}
                      >
                        <Icon
                          aria-hidden
                          className={cn("mt-0.5", iconTone[item.kind])}
                        />
                        <span className="grid min-w-0 gap-0.5">
                          <strong className="text-[13px] font-medium">
                            {words[item.kind]}
                          </strong>
                          <span className="line-clamp-2 text-xs font-normal text-muted-foreground">
                            {item.detail}
                          </span>
                          <small className="truncate font-mono text-[11px] font-normal text-faint">
                            {item.workspaceTitle}
                          </small>
                        </span>
                      </Button>
                    );
                  })}
                </section>
              ),
          )}
        </SheetContent>
      </Sheet>
    </NotificationContext>
  );
}
