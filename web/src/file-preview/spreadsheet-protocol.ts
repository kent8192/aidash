import type {
  SpreadsheetErrorCode,
  SpreadsheetKind,
  SpreadsheetWorkbook,
} from "./spreadsheet-types.ts";

export interface SpreadsheetWorkerRequest {
  id: number;
  kind: SpreadsheetKind;
  bytes: ArrayBuffer;
  fileName: string;
}

export type SpreadsheetWorkerResponse =
  | { id: number; ok: true; workbook: SpreadsheetWorkbook }
  | { id: number; ok: false; error: SpreadsheetErrorCode };
