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

/** One page of a "show more" list endpoint. */
export type Page<T> = { items: T[]; next_cursor: string | null };

/**
 * Lands a "show more" page read from `cursor` after `read` pages. The page is
 * dropped (only `loading` clears) when the pages moved on while it was in
 * flight: a re-read of the loaded range already replaced them, and that
 * re-read counted this page.
 */
export function landPage<T extends { id: string }>(
  old: WindowPages<T>,
  key: string,
  read: number,
  cursor: string | null,
  page: Page<T>,
): WindowPages<T> {
  if (old.key !== key) return old;
  if (old.read !== read || old.cursor !== cursor)
    return { ...old, loading: false };
  return {
    ...old,
    items: uniqueById([...old.items, ...page.items]),
    read: read + 1,
    cursor: page.next_cursor,
    loading: false,
  };
}

/**
 * Re-reads the loaded range behind a fresh window, starting at the window's
 * own cursor, so rows that slid out of the window are not skipped and rows
 * that left the list drop out. `depth` is read before every page rather than
 * once: a "show more" asked for while the re-read runs raises it, and the
 * re-read then covers that page on the fresh chain (the late "show more"
 * result is dropped by `landPage`, since `read` moved on).
 */
export async function rereadPages<T>(
  readPage: (cursor: string) => Promise<Page<T>>,
  windowCursor: string | null,
  depth: () => number,
): Promise<{ items: T[]; read: number; cursor: string | null }> {
  const items: T[] = [];
  let cursor = windowCursor;
  let read = 0;
  while (cursor && read < depth()) {
    const page = await readPage(cursor);
    items.push(...page.items);
    cursor = page.next_cursor;
    read += 1;
  }
  return { items, read, cursor };
}
