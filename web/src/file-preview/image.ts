import { fileExtension, normalizeMediaType } from "./preview-kind.ts";

const extensionMediaTypes: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
};

/** Image types the dashboard renders through `<img>` and a blob URL. */
const previewImageTypes = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/svg+xml",
]);

/**
 * The Blob type for an image File version. A stored `text/plain` or
 * `application/octet-stream` falls back to the file name, and SVG is always
 * labelled as SVG so it is decoded as an image, never as markup.
 */
export function imageMediaType(mediaType: string, fileName: string): string {
  const extensionType = extensionMediaTypes[fileExtension(fileName)];
  const normalized = normalizeMediaType(mediaType);
  if (previewImageTypes.has(normalized)) {
    if (fileExtension(fileName) === "svg") return "image/svg+xml";
    return normalized;
  }
  return extensionType ?? "application/octet-stream";
}

export function isPreviewImageType(
  mediaType: string,
  fileName: string,
): boolean {
  return previewImageTypes.has(imageMediaType(mediaType, fileName));
}
