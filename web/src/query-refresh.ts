/** The slice of a TanStack `QueryClient` that event-driven refreshes need. */
export interface RefreshTarget {
  isFetching(filters: { queryKey: readonly unknown[] }): number;
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
  const refresh = (key: string) => {
    if (closed) return;
    if (running.has(key)) {
      dirty.add(key);
      return;
    }
    running.add(key);
    const overlapped = target.isFetching({ queryKey: [key] }) > 0;
    void target
      .invalidateQueries({ queryKey: [key] }, { cancelRefetch: false })
      .catch(() => undefined)
      .finally(() => {
        running.delete(key);
        if (dirty.delete(key) || overlapped) refresh(key);
      });
  };
  return {
    refresh,
    close: () => {
      closed = true;
      dirty.clear();
    },
  };
}
