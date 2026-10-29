/**
 * File preview limits. The byte limit is the ADR 0068 amendment's 16 MiB for
 * every dashboard File preview. The spreadsheet limits keep a simplified
 * table view small enough to parse off the main thread and render; beyond
 * them the person downloads the file instead.
 */
export const FILE_PREVIEW_BYTE_LIMIT = 16 * 1024 * 1024;

/** Rows, including the header row, shown for one spreadsheet sheet. */
export const SPREADSHEET_ROW_LIMIT = 10_000;

/** Cells across a whole spreadsheet, counted as rows × columns per sheet. */
export const SPREADSHEET_CELL_LIMIT = 200_000;

/** Columns shown for one spreadsheet sheet. */
export const SPREADSHEET_COLUMN_LIMIT = 1_000;

export function exceedsFilePreviewLimit(bytes: ArrayBuffer): boolean {
  return bytes.byteLength > FILE_PREVIEW_BYTE_LIMIT;
}
