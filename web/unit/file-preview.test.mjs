import { test } from "node:test";
import assert from "node:assert/strict";
import ExcelJS from "exceljs";
import { detectCsvDelimiter, parseCsv } from "../src/file-preview/csv.ts";
import {
  FILE_PREVIEW_BYTE_LIMIT,
  exceedsFilePreviewLimit,
} from "../src/file-preview/limits.ts";
import {
  isHttpSafeUrl,
  parseMarkdown,
  sameRevisionImagePath,
} from "../src/file-preview/markdown.ts";
import { classifyFilePreview } from "../src/file-preview/preview-kind.ts";
import { SpreadsheetPreviewError } from "../src/file-preview/spreadsheet-types.ts";
import { parseXlsx } from "../src/file-preview/xlsx.ts";
import { resolveWorkingAreaPath } from "../src/file-preview/working-area-path.ts";
import { previewTicketErrorKey } from "../src/capabilities/preview-errors.ts";
import { remotePreviewUrl } from "../src/capabilities/remote-preview-url.ts";

test("a remote File preview opens only H's vouched one-time ticket", () => {
  const valid = {
    url: "https://a1.preview.host.example/__aidash/ticket?t=secret",
    preview_domain: "preview.host.example",
  };
  assert.equal(remotePreviewUrl(valid), valid.url);
  for (const url of [
    "https://preview.host.example.evil/__aidash/ticket?t=secret",
    "http://a1.preview.host.example/__aidash/ticket?t=secret",
    "https://a1.preview.host.example/other?t=secret",
    "https://user@a1.preview.host.example/__aidash/ticket?t=secret",
  ]) {
    assert.throws(() => remotePreviewUrl({ ...valid, url }), url);
  }
});

const bytesOf = (text) => new TextEncoder().encode(text).buffer;
const toArrayBuffer = (value) =>
  value instanceof ArrayBuffer
    ? value
    : value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength);

function spreadsheetError(code) {
  return (error) =>
    error instanceof SpreadsheetPreviewError && error.code === code;
}

test("the dispatcher chooses a renderer from the media type and extension", () => {
  const cases = [
    ["photo.png", "image/png", "image"],
    ["photo.JPG", "application/octet-stream", "image"],
    ["photo.jpeg", "text/plain", "image"],
    ["anim.gif", "image/gif", "image"],
    ["photo.webp", "image/webp", "image"],
    ["drawing.svg", "image/svg+xml", "svg"],
    ["drawing.svg", "text/plain", "svg"],
    ["README.md", "text/plain", "markdown"],
    ["README.markdown", "text/markdown; charset=utf-8", "markdown"],
    ["data.csv", "text/csv", "csv"],
    ["data.csv", "application/octet-stream", "csv"],
    [
      "book.xlsx",
      "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
      "xlsx",
    ],
    ["page.html", "text/html", "preview-domain"],
    ["paper.pdf", "application/pdf", "preview-domain"],
    ["page.htm", "text/plain", "preview-domain"],
    ["notes.txt", "text/plain", "unsupported"],
    ["archive.zip", "", "unsupported"],
    ["image-name.png", "text/html", "preview-domain"],
  ];
  for (const [fileName, mediaType, expected] of cases) {
    assert.equal(
      classifyFilePreview(fileName, mediaType),
      expected,
      `${fileName} (${mediaType})`,
    );
  }
});

test("the 16 MiB limit is inclusive and refuses every dashboard preview", () => {
  assert.equal(
    exceedsFilePreviewLimit(new ArrayBuffer(FILE_PREVIEW_BYTE_LIMIT)),
    false,
  );
  assert.equal(
    exceedsFilePreviewLimit(new ArrayBuffer(FILE_PREVIEW_BYTE_LIMIT + 1)),
    true,
  );
  assert.throws(
    () => parseCsv(new ArrayBuffer(FILE_PREVIEW_BYTE_LIMIT + 1)),
    spreadsheetError("filePreviewSpreadsheetTooLarge"),
  );
});

test("CSV handles a BOM, quoted delimiters, escaped quotes and newlines", () => {
  const csv =
    '\uFEFFname,note\r\n"Smith, J.","He said ""hi""\nnext"\r\nplain,ok\n';
  const sheet = parseCsv(bytesOf(csv)).sheets[0];
  assert.equal(sheet.name, "CSV");
  assert.deepEqual(sheet.rows, [
    ["name", "note"],
    ["Smith, J.", 'He said "hi"\nnext'],
    ["plain", "ok"],
  ]);
});

test("CSV preserves empty fields, an unclosed quote and detects the delimiter", () => {
  assert.deepEqual(parseCsv(bytesOf("a,,c")).sheets[0].rows, [["a", "", "c"]]);
  assert.deepEqual(parseCsv(bytesOf('a,"b')).sheets[0].rows, [["a", "b"]]);
  assert.deepEqual(parseCsv(bytesOf("")).sheets[0].rows, []);
  assert.equal(detectCsvDelimiter("a;b;c"), ";");
  assert.equal(detectCsvDelimiter("a\tb\tc"), "\t");
  assert.equal(detectCsvDelimiter('"a,b";c'), ";");
  assert.deepEqual(parseCsv(bytesOf("a;b\n1;2")).sheets[0].rows, [
    ["a", "b"],
    ["1", "2"],
  ]);
});

test("CSV refuses non-UTF-8 bytes instead of showing garbage", () => {
  const latin1 = new Uint8Array([0x61, 0x2c, 0xe9, 0x0a]).buffer;
  assert.throws(
    () => parseCsv(latin1),
    spreadsheetError("filePreviewSpreadsheetNotUtf8"),
  );
  const utf16 = new Uint8Array([0xff, 0xfe, 0x61, 0x00]).buffer;
  assert.throws(
    () => parseCsv(utf16),
    spreadsheetError("filePreviewSpreadsheetNotUtf8"),
  );
});

test("CSV limits stop the parse and ask for a download", () => {
  const limits = { maxRows: 1, maxCells: 10, maxColumns: 10 };
  assert.throws(
    () => parseCsv(bytesOf("a,b\nc,d"), { limits }),
    spreadsheetError("filePreviewSpreadsheetRowsLimit"),
  );
  assert.throws(
    () =>
      parseCsv(bytesOf("a,b\nc,d"), {
        limits: { maxRows: 10, maxCells: 3, maxColumns: 10 },
      }),
    spreadsheetError("filePreviewSpreadsheetCellsLimit"),
  );
  assert.throws(
    () =>
      parseCsv(bytesOf("a,b"), {
        limits: { maxRows: 10, maxCells: 10, maxColumns: 1 },
      }),
    spreadsheetError("filePreviewSpreadsheetColumnsLimit"),
  );
});

test("XLSX shows cached formula results, stored formats and merges", async () => {
  const workbook = new ExcelJS.Workbook();
  const sheet = workbook.addWorksheet("Data");
  sheet.getCell("A1").value = "label";
  sheet.getCell("A2").value = 2;
  sheet.getCell("A3").value = 3;
  sheet.getCell("A4").value = { formula: "A2+A3", result: 99 };
  sheet.getCell("A5").value = { formula: "A2+A3" };
  sheet.getCell("B1").value = {
    text: "Example",
    hyperlink: "https://example.com",
  };
  sheet.getCell("B2").value = 1234.5;
  sheet.getCell("B2").numFmt = "#,##0.00";
  sheet.getCell("B3").value = new Date(Date.UTC(2026, 9, 26));
  sheet.getCell("B3").numFmt = "yyyy-mm-dd";
  sheet.mergeCells("C1:D2");
  sheet.getCell("C1").value = "merged";
  const second = workbook.addWorksheet("Second");
  second.getCell("A1").value = "second sheet";

  const parsed = await parseXlsx(
    toArrayBuffer(await workbook.xlsx.writeBuffer()),
  );
  assert.deepEqual(
    parsed.sheets.map((parsedSheet) => parsedSheet.name),
    ["Data", "Second"],
  );
  const data = parsed.sheets[0];
  assert.equal(
    data.rows[3][0],
    "99",
    "the cached result wins over any formula",
  );
  assert.equal(
    data.rows[4][0],
    "",
    "a formula without a cached result is empty",
  );
  assert.equal(data.rows[0][1], "Example", "hyperlinks stay plain text");
  assert.equal(data.rows[1][1], "1,234.50", "number formats are applied");
  assert.equal(data.rows[2][1], "2026-10-26", "dates keep the stored instant");
  assert.deepEqual(data.merges, [
    { row: 0, column: 2, rowSpan: 2, columnSpan: 2 },
  ]);
  assert.equal(data.rows[0][2], "merged");
  assert.equal(parsed.sheets[1].rows[0][0], "second sheet");
});

test("Markdown drops raw HTML, external links and external images", () => {
  const blocks = parseMarkdown(
    [
      "# Heading",
      "",
      "<script>alert('x')</script>",
      "",
      "<b>bold</b> and <img src=x onerror=alert(1)>",
      "",
      "![alt](https://evil.test/x.png)",
      "",
      "[good](https://example.com) [bad](javascript:alert(1))",
      "",
      "![same](./img/a.png)",
      "",
      "![encoded](data:image/png;base64,AAAA)",
    ].join("\n"),
  );
  const serialized = JSON.stringify(blocks);
  assert.ok(!serialized.includes("<script"), "script tag is not emitted");
  assert.ok(!serialized.includes("<b>"), "inline tag is not emitted");
  assert.ok(!serialized.includes("onerror"), "attribute text is not emitted");
  assert.ok(serialized.includes("bold"));
  assert.ok(serialized.includes("alert('x')"), "script text stays inert text");

  const inline = blocks.flatMap((block) =>
    "children" in block ? block.children : [],
  );
  const images = inline.filter((node) => node.kind === "image");
  assert.deepEqual(images, [
    { kind: "image", path: "./img/a.png", alt: "same" },
  ]);
  const refused = inline.filter((node) => node.kind === "image-refused");
  assert.deepEqual(
    refused.map((node) => node.alt),
    ["alt", "encoded"],
  );
  const links = inline.filter((node) => node.kind === "link");
  assert.deepEqual(links, [
    { kind: "link", href: "https://example.com", text: "good" },
  ]);
  assert.equal(
    inline.some(
      (node) => node.kind === "link" && node.href.startsWith("javascript"),
    ),
    false,
  );
});

test("Markdown image and link paths only accept same-revision http(s) forms", () => {
  assert.equal(sameRevisionImagePath("./images/a.png"), "./images/a.png");
  assert.equal(sameRevisionImagePath("../a.png?v=1#x"), "../a.png");
  assert.equal(sameRevisionImagePath("https://example.com/a.png"), null);
  assert.equal(sameRevisionImagePath("//example.com/a.png"), null);
  assert.equal(sameRevisionImagePath("/etc/passwd"), null);
  assert.equal(sameRevisionImagePath("javascript:alert(1)"), null);
  assert.equal(isHttpSafeUrl("https://example.com/a"), true);
  assert.equal(isHttpSafeUrl("http://example.com/a"), true);
  assert.equal(isHttpSafeUrl("javascript:alert(1)"), false);
  assert.equal(isHttpSafeUrl("HTTPS://example.com"), true);
  assert.equal(isHttpSafeUrl("https://example.com/a b"), false);
});

test("Markdown bounds pathological syntax instead of stalling", () => {
  assert.ok(Array.isArray(parseMarkdown("[".repeat(100_000))));
  assert.ok(Array.isArray(parseMarkdown("<".repeat(100_000))));
  assert.ok(Array.isArray(parseMarkdown("*".repeat(100_000))));
  assert.ok(
    Array.isArray(
      parseMarkdown(`${">".repeat(1_000)}\n${"nested".repeat(10)}`),
    ),
  );
  const formatted = parseMarkdown("**bold** and *em* and `code`");
  assert.ok(JSON.stringify(formatted).includes('"kind":"strong"'));
  assert.ok(JSON.stringify(formatted).includes('"kind":"em"'));
});

test("Markdown image paths resolve inside the Working Area revision", () => {
  assert.equal(
    resolveWorkingAreaPath("docs/readme.md", "./img/a.png"),
    "docs/img/a.png",
  );
  assert.equal(resolveWorkingAreaPath("readme.md", "img/a.png"), "img/a.png");
  assert.equal(resolveWorkingAreaPath("docs/readme.md", "../a.png"), "a.png");
  assert.equal(
    resolveWorkingAreaPath("docs/readme.md", "img/a.png?v=1#x"),
    "docs/img/a.png",
  );
  assert.equal(resolveWorkingAreaPath("readme.md", "../a.png"), null);
  assert.equal(resolveWorkingAreaPath("docs/readme.md", "/a.png"), null);
  assert.equal(
    resolveWorkingAreaPath("docs/readme.md", "https://example.com/a.png"),
    null,
  );
  assert.equal(resolveWorkingAreaPath("docs/readme.md", "img\\a.png"), null);
});

test("preview ticket failures map to actionable states", () => {
  assert.equal(
    previewTicketErrorKey({ status: 403, message: "denied" }),
    "filePreviewNoAccessNow",
  );
  assert.equal(
    previewTicketErrorKey({ status: 404, message: "gone" }),
    "filePreviewVersionReleased",
  );
  assert.equal(
    previewTicketErrorKey({ status: 413, message: "too large" }),
    "filePreviewVersionTooLarge",
  );
  assert.equal(
    previewTicketErrorKey({ status: 503, message: "later" }),
    "filePreviewPreviewUnavailable",
  );
  assert.equal(
    previewTicketErrorKey({
      status: 409,
      message: "FILE_VERSION_TOO_LARGE",
    }),
    "filePreviewVersionTooLarge",
  );
  assert.equal(
    previewTicketErrorKey(new Error("network")),
    "filePreviewTicketError",
  );
});
