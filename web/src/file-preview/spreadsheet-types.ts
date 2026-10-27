import {
  SPREADSHEET_CELL_LIMIT,
  SPREADSHEET_COLUMN_LIMIT,
  SPREADSHEET_ROW_LIMIT,
} from "./limits.ts";

/** A merged range, with zero-based offsets into the parsed rows. */
export interface SpreadsheetMerge {
  row: number;
  column: number;
  rowSpan: number;
  columnSpan: number;
}

/** Stored cell values as text. No formulas, links, images or charts. */
export interface SpreadsheetSheet {
  name: string;
  rows: string[][];
  merges: SpreadsheetMerge[];
}

export type SpreadsheetKind = "csv" | "xlsx";

export interface SpreadsheetWorkbook {
  kind: SpreadsheetKind;
  sheets: SpreadsheetSheet[];
}

export type SpreadsheetErrorCode =
  | "filePreviewSpreadsheetTooLarge"
  | "filePreviewSpreadsheetNotUtf8"
  | "filePreviewSpreadsheetRowsLimit"
  | "filePreviewSpreadsheetCellsLimit"
  | "filePreviewSpreadsheetColumnsLimit"
  | "filePreviewSpreadsheetParseError";

export interface SpreadsheetLimits {
  maxRows: number;
  maxCells: number;
  maxColumns: number;
}

export const DEFAULT_SPREADSHEET_LIMITS: SpreadsheetLimits = {
  maxRows: SPREADSHEET_ROW_LIMIT,
  maxCells: SPREADSHEET_CELL_LIMIT,
  maxColumns: SPREADSHEET_COLUMN_LIMIT,
};

/** A typed failure the worker returns as an i18n key. */
export class SpreadsheetPreviewError extends Error {
  readonly code: SpreadsheetErrorCode;

  constructor(code: SpreadsheetErrorCode) {
    super(code);
    this.name = "SpreadsheetPreviewError";
    this.code = code;
  }
}

export function spreadsheetErrorCode(error: unknown): SpreadsheetErrorCode {
  return error instanceof SpreadsheetPreviewError
    ? error.code
    : "filePreviewSpreadsheetParseError";
}
