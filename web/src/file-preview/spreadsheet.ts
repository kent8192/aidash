import { parseCsv } from "./csv.ts";
import { parseXlsx } from "./xlsx.ts";
import type {
  SpreadsheetKind,
  SpreadsheetLimits,
  SpreadsheetWorkbook,
} from "./spreadsheet-types.ts";

/** Entry point shared by the Web Worker and the unit tests. */
export async function parseSpreadsheet(
  kind: SpreadsheetKind,
  bytes: ArrayBuffer,
  fileName: string,
  limits?: SpreadsheetLimits,
): Promise<SpreadsheetWorkbook> {
  return kind === "csv"
    ? parseCsv(bytes, { fileName, limits })
    : parseXlsx(bytes, { limits });
}
