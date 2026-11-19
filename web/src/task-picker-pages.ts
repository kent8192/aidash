import { useInfiniteQuery } from "@tanstack/react-query";
import { taskList } from "./generated/aidash";
import type { State, Task } from "./types";

/**
 * Tasks older than the `/api/state` Task window (#664), read from
 * `GET /api/tasks` only when the person asks for them. `scope` names the
 * picker, so each keeps its own pages. The listing is not filtered by Node or
 * Thread: callers keep only the Tasks they may offer.
 */
export function useTaskPages(
  data: State | undefined,
  scope: readonly unknown[],
) {
  const subject =
    data?.access.kind === "subject"
      ? `${data.access.tenant}:${data.access.subject}`
      : "";
  const page = useInfiniteQuery({
    queryKey: ["task-picker-pages", subject, ...scope],
    initialPageParam: 0,
    queryFn: ({ pageParam, signal }) =>
      taskList({ offset: pageParam }, { signal }),
    getNextPageParam: (last) => last.next_offset ?? undefined,
    enabled: false,
    retry: false,
  });
  return {
    older: (page.data?.pages.flatMap((value) => value.tasks) ?? []) as Task[],
    more: Boolean(data?.next_cursors.tasks && (!page.data || page.hasNextPage)),
    loading: page.isFetching,
    error: page.isError,
    load: () => void page.fetchNextPage(),
  };
}

/** Task pickers read older pages only when the person asks for them. */
export function useTaskPickerPages(data: State, thread: string) {
  const node = data.threads.find((item) => item.id === thread)?.node;
  const { older, ...page } = useTaskPages(data, [node, thread]);
  const tasks = [
    ...new Map(
      [...data.tasks, ...older]
        .filter((task) => task.thread_id === thread)
        .map((task) => [task.id, task]),
    ).values(),
  ];
  return { tasks, ...page };
}
