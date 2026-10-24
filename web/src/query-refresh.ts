/** The slice of a TanStack `QueryClient` that event-driven refreshes need. */
export interface RefreshTarget {
  isFetching(filters: { queryKey: readonly unknown[]; type: "active" }): number;
  invalidateQueries(
    filters: { queryKey: readonly unknown[] },
    options: { cancelRefetch: boolean },
  ): Promise<unknown>;
}

/**
 * Refreshes a query key after a server event without ever cancelling a fetch
 * that is already in flight.
 *
 * Cancelling on every event starves a query whose fetch outlasts the event
 * cadence: each restart is cancelled before it lands, and every caller
 * awaiting `invalidateQueries()` piggybacks on that endless chain. Reusing the
 * in-flight fetch alone would instead drop the event, because that response
 * may predate it and its success clears the invalidated flag. So an event that
 * overlaps a fetch earns exactly one trailing refresh once the fetch settles.
 * Only active fetches count as overlap, since `invalidateQueries` refetches
 * only active queries, and the trailing refresh never earns another: when it
 * cannot wait on the fetch (an inactive or disabled query), it settles at once
 * and re-checking overlap would spin forever.
 *
 * Returns the refresh function and a `close` that stops trailing refreshes.
 */
export function coalescedRefresh(target: RefreshTarget): {
  refresh: (key: string) => void;
  close: () => void;
} {
  const running = new Set<string>();
  const dirty = new Set<string>();
  let closed = false;
  const refresh = (key: string, trailing = false) => {
    if (closed) return;
    if (running.has(key)) {
      dirty.add(key);
      return;
    }
    running.add(key);
    const overlapped =
      !trailing && target.isFetching({ queryKey: [key], type: "active" }) > 0;
    void target
      .invalidateQueries({ queryKey: [key] }, { cancelRefetch: false })
      .catch(() => undefined)
      .finally(() => {
        running.delete(key);
        if (dirty.delete(key)) refresh(key);
        else if (overlapped) refresh(key, true);
      });
  };
  return {
    refresh: (key) => refresh(key),
    close: () => {
      closed = true;
      dirty.clear();
    },
  };
}
