import { useEffect, useRef, useState } from "react";
import {
  continuation,
  emptyPages,
  landPage,
  olderRows,
  rereadPages,
  uniqueById,
  type Page,
  type WindowPages,
} from "./window-pages";

/**
 * "Show more" pages beyond one `/api/state` window (#664), held per Node
 * (`key`). Each fresh window (`freshAt`) re-reads as many older pages as
 * were asked for, from the window's own cursor, and replaces the old ones:
 * the window's boundary moves when rows are added or leave, so pages read
 * after the old boundary would skip or keep rows. Only a key change resets
 * the pages.
 */
export function useWindowPages<T extends { id: string }>({
  key,
  windowCursor,
  freshAt,
  readPage,
  none,
}: {
  key: string;
  windowCursor: string | null;
  /** When the window was read; 0 until it is. */
  freshAt: number;
  readPage: (cursor: string, signal?: AbortSignal) => Promise<Page<T>>;
  /** A stable empty list, so memos over the rows keep their identity. */
  none: T[];
}) {
  const [pages, setPages] = useState<WindowPages<T>>(() => emptyPages(""));
  const current = pages.key === key ? pages : emptyPages<T>(key);
  const older = (pages.key === key && olderRows(pages, windowCursor)) || none;
  const cursor = continuation(current, windowCursor);
  // Pages "show more" asked for under this key, one in flight included; a
  // re-read reads that many.
  const depth = useRef({ key: "", depth: 0 });
  const reader = useRef(readPage);
  useEffect(() => {
    reader.current = readPage;
  });
  const loadMore = async () => {
    if (!cursor || current.loading) return;
    const { read } = current;
    const from = current.cursor;
    // The depth counts this page at once, so a re-read running now or
    // starting while it is in flight reads it too.
    depth.current = { key, depth: read + 1 };
    setPages({ ...current, loading: true, error: false });
    try {
      const page = await reader.current(cursor);
      setPages((old) => landPage(old, key, read, from, page));
    } catch {
      setPages((old) =>
        old.key === key ? { ...old, error: true, loading: false } : old,
      );
    }
  };
  useEffect(() => {
    if (!freshAt || depth.current.key !== key || depth.current.depth === 0)
      return;
    const controller = new AbortController();
    void (async () => {
      try {
        const { items, read, cursor } = await rereadPages(
          (from) => reader.current(from, controller.signal),
          windowCursor,
          () => (depth.current.key === key ? depth.current.depth : 0),
        );
        if (controller.signal.aborted) return;
        setPages((old) =>
          old.key === key
            ? { ...old, items: uniqueById(items), read, cursor, error: false }
            : old,
        );
      } catch {
        if (!controller.signal.aborted)
          setPages((old) => (old.key === key ? { ...old, error: true } : old));
      }
    })();
    return () => controller.abort();
    // A fresh window, not each render, starts a re-read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [freshAt, key]);
  return {
    older,
    cursor,
    loading: current.loading,
    error: current.error,
    loadMore,
  };
}
