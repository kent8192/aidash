/** User-facing notices are derived from authorized snapshots, never raw broker frames. */
export type NoticeKind = "approval" | "input" | "completed" | "failed";
export type NoticeTarget = {
  kind: "human" | "task";
  node: string;
  id: string;
  workspace: string;
};
export type NoticeEntry = {
  key: string;
  scope: string;
  signature: string;
  kind: NoticeKind | null;
  detail: string;
  workspaceTitle: string;
  target: NoticeTarget;
};
export type Notice = NoticeEntry & { kind: NoticeKind; id: string };
export type NoticeFrame = { scopes: string[]; entries: NoticeEntry[] };
export const NOTICE_INTERVAL = 30_000;
export const NOTICE_BATCH_DELAY = 1500;
export const NOTICE_HISTORY_LIMIT = 50;
const emptyView = {
  pending: [] as Notice[],
  recent: [] as Notice[],
  unread: false,
};
function notice(entry: NoticeEntry): Notice | undefined {
  return entry.kind
    ? { ...entry, kind: entry.kind, id: `${entry.key}:${entry.kind}` }
    : undefined;
}

export class NoticeStore {
  private scopes = new Set<string>();
  private previous = new Map<string, NoticeEntry>();
  private current = new Map<string, Notice>();
  private announcedInputs = new Map<string, string>();
  private recent: Notice[] = [];
  private pendingToast = new Map<string, Notice>();
  private unread = new Set<string>();
  private listeners = new Set<() => void>();
  private view = emptyView;
  private lastToast = -Infinity;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  getSnapshot = () => this.view;
  getCurrent(id: string) {
    return this.current.get(id);
  }

  observe(frame: NoticeFrame | null) {
    if (!frame) {
      this.current.clear();
      this.pendingToast.clear();
      this.recent = [];
      this.unread.clear();
      this.publish([], []);
      return;
    }
    const scopes = new Set(frame.scopes);
    const current = new Map<string, Notice>();
    const entries = new Map<string, NoticeEntry>();
    const fresh: Notice[] = [];
    for (const entry of frame.entries) {
      if (!scopes.has(entry.scope)) continue;
      entries.set(entry.key, entry);
      const item = notice(entry);
      if (!item) continue;
      current.set(item.id, item);
      const before = this.previous.get(entry.key);
      const input = item.kind === "approval" || item.kind === "input";
      const newInput = input && !this.announcedInputs.has(entry.key);
      // First snapshots, newly granted workspaces and new remote nodes are quiet.
      // A newly loaded terminal task is history, not proof of a new completion.
      if (
        this.scopes.has(entry.scope) &&
        (newInput || (!input && before && before.kind !== item.kind))
      ) {
        fresh.push(item);
      }
      if (input) this.announcedInputs.set(entry.key, entry.scope);
    }
    this.scopes = scopes;
    this.previous = entries;
    this.current = current;
    this.announcedInputs = new Map(
      [...this.announcedInputs].filter(([, scope]) => scopes.has(scope)),
    );
    this.recent = [...[...fresh].reverse(), ...this.recent]
      .filter(
        (item, index, all) =>
          item.target.kind === "task" &&
          current.has(item.id) &&
          all.findIndex((other) => other.id === item.id) === index,
      )
      .slice(0, NOTICE_HISTORY_LIMIT)
      .map((item) => current.get(item.id)!);
    for (const item of fresh) {
      this.pendingToast.set(item.id, item);
      this.unread.add(item.id);
    }
    this.pendingToast = new Map(
      [...this.pendingToast].filter(([id]) => current.has(id)),
    );
    this.unread = new Set([...this.unread].filter((id) => current.has(id)));
    this.publish(
      [...current.values()].filter((item) => item.target.kind === "human"),
      this.recent,
    );
  }
  delay(now: number) {
    return this.pendingToast.size
      ? Math.max(NOTICE_BATCH_DELAY, this.lastToast + NOTICE_INTERVAL - now)
      : undefined;
  }
  takeBatch(now: number) {
    if (now < this.lastToast + NOTICE_INTERVAL) return [];
    const batch = [...this.pendingToast.keys()].flatMap((id) => {
      const item = this.current.get(id);
      return item ? [item] : [];
    });
    this.pendingToast.clear();
    if (batch.length) this.lastToast = now;
    return batch;
  }
  markRead(id?: string) {
    if (id) {
      this.pendingToast.delete(id);
      this.unread.delete(id);
    } else {
      this.pendingToast.clear();
      this.unread.clear();
    }
    this.publish(this.view.pending, this.view.recent);
  }
  private publish(pending: Notice[], recent: Notice[]) {
    const next = { pending, recent, unread: this.unread.size > 0 };
    if (JSON.stringify(next) === JSON.stringify(this.view)) return;
    this.view = next;
    this.listeners.forEach((listener) => listener());
  }
}
