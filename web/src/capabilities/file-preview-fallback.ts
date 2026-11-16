import {
  fileExtension,
  normalizeMediaType,
} from "../file-preview/preview-kind.ts";

/** A specific MIME type takes precedence over the filename. */
export function filePreviewFallback(
  fileName: string,
  mediaType: string,
  previewAvailable: boolean,
): "pdf" | "html" | null {
  if (previewAvailable) return null;
  const type = normalizeMediaType(mediaType);
  if (type === "application/pdf") return "pdf";
  if (type === "text/html" || type === "application/xhtml+xml") return "html";
  const extension = fileExtension(fileName);
  if (extension === "pdf") return "pdf";
  if (extension === "html" || extension === "htm") return "html";
  return null;
}
