/** User-facing notices are derived from authorized snapshots, never raw broker frames. */
export type NoticeKind =
  | "approval"
  | "input"
  | "completed"
  | "failed"
  | "joined";
export type NoticeTarget = {
  kind: "human" | "task" | "approval" | "administration";
  node: string;
  id: string;
  workspace: string;
};
export type NoticeEntry = {
  eventId?: string;
  eventScope?: string;
  eventSequence?: number;
  key: string;
  scope: string;
  signature: string;
  kind: NoticeKind | null;
  detail: string;
  workspaceTitle: string;
  target: NoticeTarget;
  /** When an administration event happened, as an RFC 3339 time. */
  occurredAt?: string;
};
export type Notice = NoticeEntry & { kind: NoticeKind; id: string };
export type NoticeFrame = {
  scopes: string[];
  entries: NoticeEntry[];
  eventWatermark?: { scope: string; sequence: number };
};
/**
 * The baseline scope of a Node's task events. State events are scoped to the
 * selected Node (#664), so each Node has its own: the first frame of a Node
 * seeds its events silently instead of announcing earlier completions and
 * failures under another Node's watermark.
 */
export function taskEventScope(deployment: string, node: string) {
  return JSON.stringify([deployment, "task-events", node]);
}
export const NOTICE_INTERVAL = 30_000;
export const NOTICE_BATCH_DELAY = 1500;
export const NOTICE_HISTORY_LIMIT = 50;
const emptyView = {
  pending: [] as Notice[],
  recent: [] as Notice[],
  unread: false,
};
/**
 * The newest administration event a person has read, per scope. It outlives a
 * `NoticeStore` (a reload) and a scope missing from a failed poll, so events
 * already read are not marked unread again when they reappear.
 */
export type NoticeWatermarks = {
  get(scope: string): number | undefined;
  set(scope: string, at: number): void;
};
export function memoryWatermarks(): NoticeWatermarks {
  const marks = new Map<string, number>();
  return {
    get: (scope) => marks.get(scope),
    set: (scope, at) => void marks.set(scope, at),
  };
}
/** Watermarks kept in browser storage; unreadable storage keeps them in memory. */
export function storedWatermarks(
  storage: Pick<Storage, "getItem" | "setItem">,
  key: string,
): NoticeWatermarks {
  const marks = new Map<string, number>();
  try {
    const saved: unknown = JSON.parse(storage.getItem(key) ?? "{}");
    if (saved && typeof saved === "object")
      for (const [scope, at] of Object.entries(saved))
        if (typeof at === "number" && Number.isFinite(at)) marks.set(scope, at);
  } catch {
    // A damaged value only means old events may show as unread once more.
  }
  return {
    get: (scope) => marks.get(scope),
    set: (scope, at) => {
      marks.set(scope, at);
      try {
        storage.setItem(key, JSON.stringify(Object.fromEntries(marks)));
      } catch {
        // Storage may be full or blocked; the watermark still holds in memory.
      }
    },
  };
}
function occurred(item: Notice) {
  const at = item.occurredAt === undefined ? NaN : Date.parse(item.occurredAt);
  return Number.isNaN(at) ? undefined : at;
}
function notice(entry: NoticeEntry): Notice | undefined {
  return entry.kind
    ? { ...entry, kind: entry.kind, id: `${entry.key}:${entry.kind}` }
    : undefined;
}

export class NoticeStore {
  private seenEvents = new Set<string>();
  private seenTaskStatuses = new Set<string>();
  private eventWatermarks = new Map<string, number>();
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

  private watermarks: NoticeWatermarks;

  constructor(watermarks: NoticeWatermarks = memoryWatermarks()) {
    this.watermarks = watermarks;
  }

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
    // Missing data is a transient outage; authority changes replace the provider.
    if (!frame) return;
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
      const taskStatusRevision = JSON.stringify([item.id, entry.signature]);
      // First snapshots, newly granted workspaces and new remote nodes are quiet.
      // A newly loaded terminal task is history, not proof of a new completion.
      if (
        !this.acknowledged(item) &&
        (this.scopes.has(entry.scope) ||
          (entry.eventScope && this.scopes.has(entry.eventScope))) &&
        (newInput ||
          (!input &&
            (!entry.eventId ||
              !this.seenTaskStatuses.has(taskStatusRevision)) &&
            ((before && before.kind !== item.kind) ||
              (!before && item.target.kind === "administration") ||
              (!before &&
                entry.eventId &&
                !this.seenEvents.has(entry.eventId) &&
                (entry.eventSequence === undefined ||
                  (entry.eventScope !== undefined &&
                    entry.eventSequence >
                      (this.eventWatermarks.get(entry.eventScope) ??
                        Infinity))) &&
                !this.seenTaskStatuses.has(taskStatusRevision)))))
      ) {
        fresh.push(item);
      }
      if (input) this.announcedInputs.set(entry.key, entry.scope);
    }
    const initialAdministration = [...current.values()].filter(
      (item) =>
        item.target.kind === "administration" && !this.scopes.has(item.scope),
    );
    for (const item of initialAdministration)
      if (!this.acknowledged(item)) this.unread.add(item.id);
    for (const entry of frame.entries)
      if (entry.eventId) {
        this.seenEvents.add(entry.eventId);
        const item = notice(entry);
        if (item)
          this.seenTaskStatuses.add(JSON.stringify([item.id, entry.signature]));
      }
    if (frame.eventWatermark) {
      const { scope, sequence } = frame.eventWatermark;
      this.eventWatermarks.set(
        scope,
        Math.max(sequence, this.eventWatermarks.get(scope) ?? 0),
      );
    }
    this.scopes = scopes;
    this.previous = entries;
    this.current = current;
    this.announcedInputs = new Map(
      [...this.announcedInputs].filter(([, scope]) => scopes.has(scope)),
    );
    this.recent = [
      ...[...fresh].reverse(),
      ...initialAdministration,
      ...this.recent,
    ]
      .filter(
        (item, index, all) =>
          (item.target.kind === "task" ||
            item.target.kind === "administration") &&
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
    // Capability approvals wait on a person as human requests do.
    this.publish(
      [...current.values()].filter(
        (item) =>
          item.target.kind === "human" || item.target.kind === "approval",
      ),
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
    this.acknowledge();
    this.publish(this.view.pending, this.view.recent);
  }
  /** An administration event at or before its scope's watermark was read. */
  private acknowledged(item: Notice) {
    if (item.target.kind !== "administration") return false;
    const at = occurred(item);
    const mark = this.watermarks.get(item.scope);
    return at !== undefined && mark !== undefined && at <= mark;
  }
  /**
   * Advance each scope's watermark to its newest read event that is older than
   * every unread one, so reading one event never acknowledges an older one.
   */
  private acknowledge() {
    const administration = [...this.current.values()].filter(
      (item) =>
        item.target.kind === "administration" && occurred(item) !== undefined,
    );
    const oldestUnread = new Map<string, number>();
    for (const item of administration)
      if (this.unread.has(item.id))
        oldestUnread.set(
          item.scope,
          Math.min(occurred(item)!, oldestUnread.get(item.scope) ?? Infinity),
        );
    const newestRead = new Map<string, number>();
    for (const item of administration) {
      const at = occurred(item)!;
      if (
        !this.unread.has(item.id) &&
        at < (oldestUnread.get(item.scope) ?? Infinity)
      )
        newestRead.set(
          item.scope,
          Math.max(at, newestRead.get(item.scope) ?? -Infinity),
        );
    }
    for (const [scope, at] of newestRead)
      if (at > (this.watermarks.get(scope) ?? -Infinity))
        this.watermarks.set(scope, at);
  }
  private publish(pending: Notice[], recent: Notice[]) {
    const next = { pending, recent, unread: this.unread.size > 0 };
    if (JSON.stringify(next) === JSON.stringify(this.view)) return;
    this.view = next;
    this.listeners.forEach((listener) => listener());
  }
}
