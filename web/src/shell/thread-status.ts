import { taskProgress } from "../collaboration/model.ts";

export type ThreadTone = "attention" | "active" | "done" | "idle" | "unknown";

/**
 * What the Thread list may say about one Thread from the Node-wide
 * windows of `/api/state` (#664). Tasks and pending requests found in the
 * windows prove attention, activity and waiting decisions. Completion and
 * idleness need every Task: while the Task window has more (`moreTasks`), a
 * Thread's older Tasks may still be open, so the tone is "unknown" and no
 * count is given. While more pending requests exist (`moreRequests`), the
 * waiting count is a lower bound.
 */
export function threadStatus(
  tasks: Parameters<typeof taskProgress>[0],
  requests: readonly { thread_id: string; response: unknown }[],
  thread: string,
  { moreTasks, moreRequests }: { moreTasks: boolean; moreRequests: boolean },
): {
  tone: ThreadTone;
  progress: { completed: number; total: number } | null;
  pending: number;
  pendingMore: boolean;
} {
  const progress = taskProgress(tasks, thread);
  const pending = requests.filter(
    (request) => request.thread_id === thread && request.response === null,
  ).length;
  const tone: ThreadTone =
    progress.attention > 0
      ? "attention"
      : progress.active > 0
        ? "active"
        : moreTasks
          ? "unknown"
          : progress.total > 0 && progress.completed === progress.total
            ? "done"
            : "idle";
  return {
    tone,
    progress:
      !moreTasks && progress.total > 0
        ? { completed: progress.completed, total: progress.total }
        : null,
    pending,
    pendingMore: moreRequests,
  };
}
