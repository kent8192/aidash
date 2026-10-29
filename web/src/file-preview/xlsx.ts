import ExcelJS from "exceljs";
import { FILE_PREVIEW_BYTE_LIMIT } from "./limits.ts";
import {
  DEFAULT_SPREADSHEET_LIMITS,
  SpreadsheetPreviewError,
  type SpreadsheetLimits,
  type SpreadsheetMerge,
  type SpreadsheetSheet,
  type SpreadsheetWorkbook,
} from "./spreadsheet-types.ts";

export interface XlsxParseOptions {
  limits?: SpreadsheetLimits;
}

/**
 * Reads an XLSX File version with ExcelJS and keeps only stored cell values.
 * ExcelJS does not calculate formulas, so a formula cell shows its cached
 * result; a formula without one shows as empty. Charts, images, macros and
 * hyperlink targets are not part of the result at all.
 */
export async function parseXlsx(
  bytes: ArrayBuffer,
  options: XlsxParseOptions = {},
): Promise<SpreadsheetWorkbook> {
  if (bytes.byteLength > FILE_PREVIEW_BYTE_LIMIT) {
    throw new SpreadsheetPreviewError("filePreviewSpreadsheetTooLarge");
  }
  const limits = options.limits ?? DEFAULT_SPREADSHEET_LIMITS;
  const workbook = new ExcelJS.Workbook();
  try {
    await workbook.xlsx.load(bytes);
  } catch {
    throw new SpreadsheetPreviewError("filePreviewSpreadsheetParseError");
  }

  const sheets: SpreadsheetSheet[] = [];
  workbook.eachSheet((worksheet) => {
    const rowCount = worksheet.rowCount;
    const columnCount = worksheet.columnCount;
    if (rowCount > limits.maxRows) {
      throw new SpreadsheetPreviewError("filePreviewSpreadsheetRowsLimit");
    }
    if (columnCount > limits.maxColumns) {
      throw new SpreadsheetPreviewError("filePreviewSpreadsheetColumnsLimit");
    }
    if (rowCount * columnCount > limits.maxCells) {
      throw new SpreadsheetPreviewError("filePreviewSpreadsheetCellsLimit");
    }

    const rows: string[][] = [];
    for (let rowNumber = 1; rowNumber <= rowCount; rowNumber += 1) {
      const row = worksheet.getRow(rowNumber);
      const values: string[] = [];
      for (let column = 1; column <= columnCount; column += 1) {
        values.push(cellText(row.getCell(column)));
      }
      rows.push(values);
    }

    sheets.push({
      name: worksheet.name,
      rows,
      merges: worksheetMerges(worksheet, rowCount, columnCount),
    });
  });

  return { kind: "xlsx", sheets };
}

function worksheetMerges(
  worksheet: ExcelJS.Worksheet,
  rowCount: number,
  columnCount: number,
): SpreadsheetMerge[] {
  const ranges = (worksheet.model?.merges ?? []) as string[];
  const merges: SpreadsheetMerge[] = [];
  for (const range of ranges) {
    const [startText, endText] = range.split(":");
    const start = parseAddress(startText);
    const end = parseAddress(endText ?? startText);
    if (!start || !end) continue;
    if (start.row > rowCount || start.column > columnCount) continue;
    merges.push({
      row: start.row - 1,
      column: start.column - 1,
      rowSpan: Math.min(end.row, rowCount) - start.row + 1,
      columnSpan: Math.min(end.column, columnCount) - start.column + 1,
    });
  }
  return merges;
}

function parseAddress(
  address: string | undefined,
): { row: number; column: number } | null {
  if (!address) return null;
  const match = /^\$?([A-Za-z]{1,3})\$?([1-9][0-9]*)$/.exec(address.trim());
  if (!match) return null;
  let column = 0;
  for (const char of match[1].toUpperCase()) {
    column = column * 26 + char.charCodeAt(0) - 64;
  }
  return { row: Number(match[2]), column };
}

/**
 * Stored value as text. Formula cells use their cached result, dates use the
 * stored instant and simple number formats are applied; anything ExcelJS
 * cannot describe faithfully stays the plain stored value.
 */
export function cellText(cell: ExcelJS.Cell): string {
  return formatStoredValue(cell.value, cell.numFmt);
}

export function formatStoredValue(
  value: ExcelJS.CellValue,
  numFmt: string | undefined,
): string {
  if (value === null || value === undefined) return "";
  if (value instanceof Date) return formatDate(value, numFmt);
  if (typeof value === "number") return formatNumber(value, numFmt);
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  if (typeof value === "string") return value;
  if (typeof value === "object") {
    const record = value as unknown as Record<string, unknown>;
    if ("formula" in record || "sharedFormula" in record) {
      const result = record.result;
      return result === null || result === undefined
        ? ""
        : formatStoredValue(result as ExcelJS.CellValue, numFmt);
    }
    if (Array.isArray(record.richText)) {
      return record.richText
        .map((part) =>
          part && typeof part === "object" && "text" in part
            ? String((part as { text?: unknown }).text ?? "")
            : "",
        )
        .join("");
    }
    if (typeof record.text === "string") return record.text;
    if (typeof record.error === "string") return record.error;
  }
  return String(value);
}

function formatDate(value: Date, numFmt: string | undefined): string {
  const iso = value.toISOString();
  const date = iso.slice(0, 10);
  const time = iso.slice(11, 19);
  const wantsTime = /[hs]/i.test(numFmt ?? "") || time !== "00:00:00";
  return wantsTime ? `${date} ${time}` : date;
}

function formatNumber(value: number, numFmt: string | undefined): string {
  const format = (numFmt ?? "").trim();
  if (!format) return String(value);
  const decimals = fractionDigits(format);
  try {
    if (format.includes("%")) {
      return `${numberFormat(decimals, false).format(value * 100)}%`;
    }
    if (/[0#]/.test(format) && !format.includes(";")) {
      return numberFormat(decimals, format.includes(",")).format(value);
    }
  } catch {
    return String(value);
  }
  return String(value);
}

function numberFormat(
  fractionDigits: number,
  grouping: boolean,
): Intl.NumberFormat {
  return new Intl.NumberFormat("en-US", {
    useGrouping: grouping,
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  });
}

function fractionDigits(format: string): number {
  const dot = format.lastIndexOf(".");
  if (dot < 0) return 0;
  let digits = 0;
  for (let index = dot + 1; index < format.length; index += 1) {
    const char = format[index];
    if (char === "0" || char === "#") digits += 1;
    else break;
  }
  return Math.min(digits, 20);
}
