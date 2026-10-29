/**
 * Which File preview, if any, the dashboard can render for a File version.
 * HTML and PDF are distinct: their File preview is served from the preview
 * domain by the Preview gateway, not from the dashboard.
 */
export type FilePreviewKind =
  | "image"
  | "svg"
  | "markdown"
  | "csv"
  | "xlsx"
  | "preview-domain"
  | "unsupported";

const imageMediaTypes = new Set([
  "image/png",
  "image/jpeg",
  "image/jpg",
  "image/gif",
  "image/webp",
]);

const markdownMediaTypes = new Set([
  "text/markdown",
  "text/x-markdown",
  "application/markdown",
]);

const csvMediaTypes = new Set([
  "text/csv",
  "application/csv",
  "application/x-csv",
]);

const previewDomainMediaTypes = new Set([
  "text/html",
  "application/xhtml+xml",
  "application/pdf",
]);

const xlsxMediaType =
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

const extensionKinds: Record<string, FilePreviewKind> = {
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  svg: "svg",
  md: "markdown",
  markdown: "markdown",
  csv: "csv",
  xlsx: "xlsx",
  htm: "preview-domain",
  html: "preview-domain",
  pdf: "preview-domain",
};

export function normalizeMediaType(mediaType: string): string {
  return mediaType.split(";", 1)[0]?.trim().toLowerCase() ?? "";
}

/** The lower-case extension without its dot, or an empty string. */
export function fileExtension(fileName: string): string {
  const base = fileName.split(/[\\/]/).pop() ?? "";
  const dot = base.lastIndexOf(".");
  if (dot <= 0 || dot === base.length - 1) return "";
  return base.slice(dot + 1).toLowerCase();
}

function kindForMediaType(mediaType: string): FilePreviewKind | null {
  if (previewDomainMediaTypes.has(mediaType)) return "preview-domain";
  if (mediaType === "image/svg+xml") return "svg";
  if (imageMediaTypes.has(mediaType)) return "image";
  if (markdownMediaTypes.has(mediaType)) return "markdown";
  if (csvMediaTypes.has(mediaType)) return "csv";
  if (mediaType === xlsxMediaType) return "xlsx";
  return null;
}

/**
 * Chooses a renderer from the media type and, when the media type is generic
 * or unknown, from the file name. A specific media type wins: a version
 * labelled `text/html` is never handed to the image renderer just because
 * its name ends in `.png`.
 */
export function classifyFilePreview(
  fileName: string,
  mediaType: string,
): FilePreviewKind {
  const normalized = normalizeMediaType(mediaType);
  const byMediaType = kindForMediaType(normalized);
  if (byMediaType) return byMediaType;
  const byExtension = extensionKinds[fileExtension(fileName)];
  if (byExtension) return byExtension;
  return "unsupported";
}

export function isImageKind(kind: FilePreviewKind): boolean {
  return kind === "image" || kind === "svg";
}

export function isSpreadsheetKind(
  kind: FilePreviewKind,
): kind is "csv" | "xlsx" {
  return kind === "csv" || kind === "xlsx";
}
