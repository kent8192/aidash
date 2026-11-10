import { useInfiniteQuery } from "@tanstack/react-query";
import { taskList } from "./generated/aidash";
import type { State, Task } from "./types";

/** Task pickers read older pages only when the person asks for them. */
export function useTaskPickerPages(data: State, workspace: string) {
  const node = data.workspaces.find((item) => item.id === workspace)?.node;
  const subject =
    data.access.kind === "subject"
      ? `${data.access.tenant}:${data.access.subject}`
      : "";
  const page = useInfiniteQuery({
    queryKey: ["task-picker-pages", subject, node, workspace],
    initialPageParam: 0,
    queryFn: ({ pageParam, signal }) =>
      taskList({ offset: pageParam }, { signal }),
    getNextPageParam: (last) => last.next_offset ?? undefined,
    enabled: false,
    retry: false,
  });
  const tasks = [
    ...new Map(
      [
        ...data.tasks,
        ...(page.data?.pages.flatMap((value) => value.tasks) ?? []),
      ]
        .filter((task) => task.workspace_id === workspace)
        .map((task) => [task.id, task]),
    ).values(),
  ] as Task[];
  return {
    tasks,
    more: Boolean(data.next_cursors.tasks && (!page.data || page.hasNextPage)),
    loading: page.isFetching,
    error: page.isError,
    load: () => void page.fetchNextPage(),
  };
}
