/**
 * Rows of a fresh `/api/state` window joined with the older rows that
 * "show more" loaded beyond it (#664). The fresh window comes first and wins
 * for an id both hold, so a re-read never shows a stale copy.
 */
export function mergeWindow<T extends { id: string }>(
  fresh: readonly T[],
  older: readonly T[],
): T[] {
  const ids = new Set(fresh.map((row) => row.id));
  return [...fresh, ...older.filter((row) => !ids.has(row.id))];
}

/**
 * One copy of each id, in the order ids first appear; the last copy read
 * wins. Keyset pages ordered by a changing column can return a row twice.
 */
export function uniqueById<T extends { id: string }>(rows: Iterable<T>): T[] {
  return [...new Map([...rows].map((row) => [row.id, row])).values()];
}

/** A continuation of one `/api/state` window, held per Node. */
export type WindowPages<T> = {
  /** The Node (and dashboard context) the pages belong to. */
  key: string;
  /** Rows read beyond the window, oldest last. */
  items: T[];
  /** Pages read so far. */
  read: number;
  /** Where the page after `items` starts; meaningful once `read > 0`. */
  cursor: string | null;
  loading: boolean;
  error: boolean;
};

export function emptyPages<T>(key: string): WindowPages<T> {
  return {
    key,
    items: [],
    read: 0,
    cursor: null,
    loading: false,
    error: false,
  };
}

/**
 * Where the next "show more" page starts: the fresh window's own cursor until
 * a page is loaded, then the cursor of the last page read. A window without a
 * cursor holds everything, so nothing continues it.
 */
export function continuation(
  pages: Pick<WindowPages<unknown>, "read" | "cursor">,
  windowCursor: string | null,
): string | null {
  if (windowCursor === null) return null;
  return pages.read > 0 ? pages.cursor : windowCursor;
}

/**
 * The loaded rows still worth showing beside a fresh window: none once the
 * window holds everything (its cursor is null), since a row missing from a
 * complete window is gone (for pending requests: answered).
 */
export function olderRows<T>(
  pages: Pick<WindowPages<T>, "items">,
  windowCursor: string | null,
): T[] | null {
  return windowCursor === null ? null : pages.items;
}
