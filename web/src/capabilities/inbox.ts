import { useQuery } from "@tanstack/react-query";
import { apiFetch } from "../transport";
import type { ApprovalCard } from "../generated/models";

/** Every page of one filtered approvals list; an inbox must not stop at page one. */
export async function every(path: string, signal: AbortSignal) {
  const items: ApprovalCard[] = [];
  let cursor: string | null = null;
  do {
    const page: { items: ApprovalCard[]; next_cursor: string | null } =
      await apiFetch(
        `${path}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`,
        { signal },
      );
    items.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor);
  return items;
}

/**
 * The signed-in approver's inbox: capability approvals awaiting their
 * decision and the unexpired Run grants they gave. A designated approver may
 * be unable to read the Thread, so decisions and revocations live here.
 * Keys start with `core-approvals`, so every decision refreshes them.
 */
export function useApprovalInbox(enabled: boolean) {
  const pending = useQuery({
    queryKey: ["core-approvals", "inbox", "pending"],
    queryFn: ({ signal }) =>
      every("/api/capabilities/approvals?state=pending&role=approver", signal),
    refetchInterval: 5000,
    retry: false,
    enabled,
  });
  const grants = useQuery({
    queryKey: ["core-approvals", "inbox", "grants"],
    queryFn: ({ signal }) =>
      every("/api/capabilities/approvals?state=active&role=approver", signal),
    refetchInterval: 5000,
    retry: false,
    enabled,
  });
  // The server already filters; the same rule here keeps the inbox exact.
  return {
    pending: pending.data?.filter(
      (item) => item.state === "pending" && item.viewer_is_approver,
    ),
    grants: grants.data?.filter(
      (item) =>
        item.kind === "grant" &&
        item.state === "active" &&
        item.viewer_is_approver,
    ),
  };
}
