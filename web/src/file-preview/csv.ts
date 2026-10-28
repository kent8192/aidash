import { FILE_PREVIEW_BYTE_LIMIT } from "./limits.ts";
import {
  DEFAULT_SPREADSHEET_LIMITS,
  SpreadsheetPreviewError,
  type SpreadsheetLimits,
  type SpreadsheetWorkbook,
} from "./spreadsheet-types.ts";
import { decodeUtf8, hasUtf16Bom } from "./text.ts";

export interface CsvParseOptions {
  fileName?: string;
  limits?: SpreadsheetLimits;
}

const candidates = [",", ";", "\t"] as const;

/**
 * Picks the delimiter from the first line, ignoring quoted runs. Comma wins
 * ties, so a plain CSV keeps its usual meaning.
 */
export function detectCsvDelimiter(text: string): string {
  const firstLine = text.split(/\r?\n/, 1)[0] ?? "";
  const counts = new Map<string, number>(candidates.map((value) => [value, 0]));
  let quoted = false;
  for (let index = 0; index < firstLine.length; index += 1) {
    const char = firstLine[index];
    if (char === '"') {
      if (quoted && firstLine[index + 1] === '"') {
        index += 1;
      } else {
        quoted = !quoted;
      }
      continue;
    }
    if (!quoted) {
      const count = counts.get(char);
      if (count !== undefined) counts.set(char, count + 1);
    }
  }
  let delimiter = ",";
  let best = 0;
  for (const candidate of candidates) {
    const count = counts.get(candidate) ?? 0;
    if (count > best) {
      delimiter = candidate;
      best = count;
    }
  }
  return delimiter;
}

/**
 * Parses a CSV File version into plain rows of stored text. Quoted fields
 * may contain the delimiter, doubled quotes and newlines. The parser stops
 * with a typed error as soon as a limit is exceeded; it never renders a
 * silently truncated sheet as if it were complete.
 */
export function parseCsv(
  bytes: ArrayBuffer,
  options: CsvParseOptions = {},
): SpreadsheetWorkbook {
  if (bytes.byteLength > FILE_PREVIEW_BYTE_LIMIT) {
    throw new SpreadsheetPreviewError("filePreviewSpreadsheetTooLarge");
  }
  if (hasUtf16Bom(bytes) || containsNul(bytes)) {
    throw new SpreadsheetPreviewError("filePreviewSpreadsheetNotUtf8");
  }
  const decoded = decodeUtf8(bytes);
  if (!decoded.ok) {
    throw new SpreadsheetPreviewError("filePreviewSpreadsheetNotUtf8");
  }
  const limits = options.limits ?? DEFAULT_SPREADSHEET_LIMITS;
  const text = decoded.text;
  const sheetName = options.fileName?.trim() || "CSV";
  if (text.length === 0) {
    return { kind: "csv", sheets: [{ name: sheetName, rows: [], merges: [] }] };
  }

  const delimiter = detectCsvDelimiter(text);
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  let cells = 0;
  let endedWithRowBreak = false;

  const endField = () => {
    row.push(field);
    field = "";
    cells += 1;
    if (cells > limits.maxCells) {
      throw new SpreadsheetPreviewError("filePreviewSpreadsheetCellsLimit");
    }
    if (row.length > limits.maxColumns) {
      throw new SpreadsheetPreviewError("filePreviewSpreadsheetColumnsLimit");
    }
  };
  const endRow = () => {
    endField();
    rows.push(row);
    row = [];
    if (rows.length > limits.maxRows) {
      throw new SpreadsheetPreviewError("filePreviewSpreadsheetRowsLimit");
    }
    endedWithRowBreak = true;
  };

  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (quoted) {
      if (char === '"') {
        if (text[index + 1] === '"') {
          field += '"';
          index += 1;
        } else {
          quoted = false;
        }
      } else {
        field += char;
      }
      continue;
    }
    if (char === '"' && field.length === 0) {
      quoted = true;
      continue;
    }
    if (char === delimiter) {
      endField();
      endedWithRowBreak = false;
      continue;
    }
    if (char === "\r" || char === "\n") {
      if (char === "\r" && text[index + 1] === "\n") index += 1;
      endRow();
      continue;
    }
    field += char;
    endedWithRowBreak = false;
  }
  if (field.length > 0 || row.length > 0 || !endedWithRowBreak) endRow();

  return { kind: "csv", sheets: [{ name: sheetName, rows, merges: [] }] };
}

function containsNul(bytes: ArrayBuffer): boolean {
  const sample = new Uint8Array(bytes, 0, Math.min(bytes.byteLength, 4096));
  return sample.includes(0);
}
