import { useEffect, useRef, useState } from "react";
import { Alert, Hint, Loading, Notice } from "../components/patterns";
import { Button } from "../components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../components/ui/table";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../components/ui/tabs";
import { cn } from "../lib/utils";
import { useI18n } from "../ui";
import { exceedsFilePreviewLimit } from "./limits.ts";
import SpreadsheetWorker from "./spreadsheet.worker?worker";
import type {
  SpreadsheetWorkerRequest,
  SpreadsheetWorkerResponse,
} from "./spreadsheet-protocol.ts";
import {
  type SpreadsheetErrorCode,
  type SpreadsheetMerge,
  type SpreadsheetSheet,
  type SpreadsheetWorkbook,
} from "./spreadsheet-types.ts";
import type { FilePreviewVersion } from "./types.ts";

export interface SpreadsheetFilePreviewProps extends FilePreviewVersion {
  kind: "csv" | "xlsx";
  className?: string;
  onDownload?: (version: FilePreviewVersion) => void;
}

type SpreadsheetState =
  | { status: "loading"; key: string }
  | { status: "ready"; key: string; workbook: SpreadsheetWorkbook }
  | { status: "error"; key: string; code: SpreadsheetErrorCode };

const bufferIds = new WeakMap<ArrayBuffer, number>();
let nextBufferId = 1;

function requestKey(
  bytes: ArrayBuffer,
  kind: "csv" | "xlsx",
  fileName: string,
): string {
  let id = bufferIds.get(bytes);
  if (id === undefined) {
    id = nextBufferId;
    nextBufferId += 1;
    bufferIds.set(bytes, id);
  }
  return `${id}:${kind}:${fileName}`;
}

/**
 * CSV and XLSX simplified table view. Parsing happens in a Worker so a large
 * File version never blocks the thread; the result is stored values only,
 * with an explicit notice that formulas are not recalculated and that
 * charts, images, macros and active links are not shown.
 */
export function SpreadsheetFilePreview({
  kind,
  bytes,
  mediaType,
  fileName,
  className,
  onDownload,
}: SpreadsheetFilePreviewProps) {
  const { t } = useI18n();
  const requestId = useRef(0);
  const key = requestKey(bytes, kind, fileName);
  const tooLarge = exceedsFilePreviewLimit(bytes);
  const [state, setState] = useState<SpreadsheetState>(() =>
    tooLarge
      ? { status: "error", key, code: "filePreviewSpreadsheetTooLarge" }
      : { status: "loading", key },
  );

  useEffect(() => {
    if (tooLarge) return;
    let cancelled = false;
    const id = requestId.current + 1;
    requestId.current = id;
    const worker = new SpreadsheetWorker();
    worker.onmessage = (event: MessageEvent<SpreadsheetWorkerResponse>) => {
      if (cancelled || event.data.id !== id) return;
      if (event.data.ok) {
        setState({ status: "ready", key, workbook: event.data.workbook });
      } else {
        setState({ status: "error", key, code: event.data.error });
      }
      worker.terminate();
    };
    worker.onerror = (event) => {
      event.preventDefault();
      if (cancelled) return;
      setState({
        status: "error",
        key,
        code: "filePreviewSpreadsheetParseError",
      });
      worker.terminate();
    };
    try {
      const copy = bytes.slice(0);
      const request: SpreadsheetWorkerRequest = {
        id,
        kind,
        bytes: copy,
        fileName,
      };
      worker.postMessage(request, [copy]);
    } catch {
      queueMicrotask(() => {
        if (cancelled) return;
        setState({
          status: "error",
          key,
          code: "filePreviewSpreadsheetParseError",
        });
        worker.terminate();
      });
    }
    return () => {
      cancelled = true;
      worker.terminate();
    };
  }, [bytes, key, kind, fileName, tooLarge]);

  if (tooLarge) {
    return (
      <div className={cn("grid min-w-0 gap-2", className)}>
        <Alert>{t("filePreviewSpreadsheetTooLarge")}</Alert>
        {onDownload && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            onClick={() => onDownload({ bytes, mediaType, fileName })}
          >
            {t("filePreviewDownload")}
          </Button>
        )}
      </div>
    );
  }
  const current =
    state.key === key ? state : { status: "loading" as const, key };
  if (current.status === "loading") {
    return <Loading>{t("filePreviewSpreadsheetLoading")}</Loading>;
  }
  if (current.status === "error") {
    return (
      <div className={cn("grid min-w-0 gap-2", className)}>
        <Alert>{t(current.code)}</Alert>
        {onDownload && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            onClick={() => onDownload({ bytes, mediaType, fileName })}
          >
            {t("filePreviewDownload")}
          </Button>
        )}
      </div>
    );
  }

  const sheets = current.workbook.sheets;
  return (
    <div className={cn("grid min-w-0 gap-2", className)}>
      <Notice>{t("filePreviewSpreadsheetSimplified")}</Notice>
      {sheets.length === 0 ? (
        <Hint>{t("filePreviewSpreadsheetEmpty")}</Hint>
      ) : sheets.length === 1 ? (
        <SheetTable sheet={sheets[0]} />
      ) : (
        <Tabs
          key={sheets.map((sheet) => sheet.name).join("\u0000")}
          defaultValue={sheets[0].name}
        >
          <TabsList aria-label={t("filePreviewSpreadsheetTabs")}>
            {sheets.map((sheet) => (
              <TabsTrigger key={sheet.name} value={sheet.name}>
                {sheet.name}
              </TabsTrigger>
            ))}
          </TabsList>
          {sheets.map((sheet) => (
            <TabsContent key={sheet.name} value={sheet.name} className="mt-2">
              <SheetTable sheet={sheet} />
            </TabsContent>
          ))}
        </Tabs>
      )}
    </div>
  );
}

function SheetTable({ sheet }: { sheet: SpreadsheetSheet }) {
  const { t } = useI18n();
  if (sheet.rows.length === 0) {
    return <Hint>{t("filePreviewSpreadsheetEmpty")}</Hint>;
  }
  const [header, ...body] = sheet.rows;
  const columnCount = sheet.rows.reduce(
    (maximum, row) => Math.max(maximum, row.length),
    0,
  );
  const merges = mergeIndex(sheet.merges);
  const headerRow = (
    <TableRow>
      {Array.from({ length: columnCount }, (_, column) => {
        if (merges.covered.has(`${0}:${column}`)) return null;
        const merge = merges.at.get(`${0}:${column}`);
        return (
          <TableHead
            key={column}
            rowSpan={merge?.rowSpan}
            colSpan={merge?.columnSpan}
          >
            {header?.[column] ?? ""}
          </TableHead>
        );
      })}
    </TableRow>
  );
  return (
    <div className="max-h-[60vh] overflow-auto rounded-md border border-border">
      <Table aria-label={sheet.name}>
        <TableHeader>{headerRow}</TableHeader>
        <TableBody>
          {body.map((row, rowIndex) => {
            const rowNumber = rowIndex + 1;
            return (
              <TableRow key={rowNumber}>
                {Array.from({ length: columnCount }, (_, column) => {
                  if (merges.covered.has(`${rowNumber}:${column}`)) {
                    return null;
                  }
                  const merge = merges.at.get(`${rowNumber}:${column}`);
                  return (
                    <TableCell
                      key={column}
                      rowSpan={merge?.rowSpan}
                      colSpan={merge?.columnSpan}
                      className="max-w-[40ch] whitespace-pre-wrap align-top"
                    >
                      {row[column] ?? ""}
                    </TableCell>
                  );
                })}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}

function mergeIndex(merges: SpreadsheetMerge[]): {
  at: Map<string, SpreadsheetMerge>;
  covered: Set<string>;
} {
  const at = new Map<string, SpreadsheetMerge>();
  const covered = new Set<string>();
  for (const merge of merges) {
    at.set(`${merge.row}:${merge.column}`, merge);
    for (let row = merge.row; row < merge.row + merge.rowSpan; row += 1) {
      for (
        let column = merge.column;
        column < merge.column + merge.columnSpan;
        column += 1
      ) {
        if (row === merge.row && column === merge.column) continue;
        covered.add(`${row}:${column}`);
      }
    }
  }
  return { at, covered };
}
