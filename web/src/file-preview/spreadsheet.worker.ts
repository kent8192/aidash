import { parseSpreadsheet } from "./spreadsheet.ts";
import type {
  SpreadsheetWorkerRequest,
  SpreadsheetWorkerResponse,
} from "./spreadsheet-protocol.ts";
import { spreadsheetErrorCode } from "./spreadsheet-types.ts";

/**
 * Parses CSV and XLSX File versions off the main thread. ExcelJS is large, so
 * this Worker is created only when a spreadsheet File preview opens and is
 * terminated when the preview closes.
 */
interface WorkerScope {
  onmessage: ((event: MessageEvent<SpreadsheetWorkerRequest>) => void) | null;
  postMessage: (message: SpreadsheetWorkerResponse) => void;
}

const scope = self as unknown as WorkerScope;

scope.onmessage = (event) => {
  const { id, kind, bytes, fileName } = event.data;
  void parseSpreadsheet(kind, bytes, fileName).then(
    (workbook) => scope.postMessage({ id, ok: true, workbook }),
    (error: unknown) =>
      scope.postMessage({ id, ok: false, error: spreadsheetErrorCode(error) }),
  );
};
