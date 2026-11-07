/** One document found in a chosen folder. */
export type FolderFile<F = unknown> = {
  /** Path inside the folder, without the folder's own name. */
  path: string;
  size: number;
  file: F;
};

/** Base64 grows 4/3, so this keeps a request under the server's 16 MiB limit. */
export const MAX_BATCH_BYTES = 11 * 1024 * 1024;
/** The server's limits for one Reference file and one Reference set. */
export const MAX_FILE_BYTES = 10 * 1024 * 1024;
export const MAX_REFERENCE_FILES = 256;

const isHidden = (path: string) =>
  path.split("/").some((segment) => segment.startsWith("."));

/**
 * The documents a folder offers: hidden files and folders such as `.git`, and
 * empty files such as `.gitkeep`, never become Reference files.
 */
export function folderFiles<F>(
  files: readonly { relativePath: string; size: number; file: F }[],
): FolderFile<F>[] {
  return files
    .map(({ relativePath, size, file }) => ({
      path: relativePath.split("/").slice(1).join("/"),
      size,
      file,
    }))
    .filter(({ path, size }) => path && size > 0 && !isHidden(path))
    .sort((a, b) => a.path.localeCompare(b.path));
}

/** Groups documents into requests the server accepts, keeping their order. */
export function batches<T extends { size: number }>(
  files: readonly T[],
  maximum = MAX_BATCH_BYTES,
  maximumFiles = MAX_REFERENCE_FILES,
): T[][] {
  const result: T[][] = [];
  let current: T[] = [];
  let bytes = 0;
  for (const file of files) {
    if (
      current.length &&
      (bytes + file.size > maximum || current.length >= maximumFiles)
    ) {
      result.push(current);
      current = [];
      bytes = 0;
    }
    current.push(file);
    bytes += file.size;
  }
  if (current.length) result.push(current);
  return result;
}

/** Tells a person which pages of a PDF an Agent cannot read. */
export function pagesWithoutTextNotice(
  pages: readonly number[] | undefined,
  pageCount: number | null | undefined,
  japanese: boolean,
): string | undefined {
  if (!pages?.length) return undefined;
  if (pages.length === pageCount)
    return japanese
      ? `全 ${pageCount} ページに文字がありません。スキャンした PDF の可能性があり、Agent は中身を読めません。`
      : `None of the ${pageCount} pages has text. This may be a scanned PDF, and an Agent cannot read it.`;
  const list = pages.join(", ");
  return japanese
    ? `${pageCount ?? "?"} ページ中 ${list} ページは文字が読めません。`
    : `Pages ${list} of ${pageCount ?? "?"} have no text an Agent can read.`;
}

/**
 * Adds entries to a Source's list. A Source binds a Reference once and a path
 * once, so a later entry replaces any earlier one with the same id or path.
 */
export function mergeReferences<
  T extends { reference_id: string; path: string },
>(current: readonly T[], added: readonly T[]): T[] {
  const merged: T[] = [];
  for (const entry of [...current, ...added]) {
    const clash = merged.findIndex(
      (other) =>
        other.reference_id === entry.reference_id || other.path === entry.path,
    );
    if (clash >= 0) merged.splice(clash, 1);
    merged.push(entry);
  }
  return merged;
}
