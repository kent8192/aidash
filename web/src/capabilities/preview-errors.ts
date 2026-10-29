/**
 * Maps a preview ticket or Preview gateway failure to the i18n key the pane
 * shows. Kept dependency-free so node:test can cover it.
 */
export function previewTicketErrorKey(error: unknown): string {
  const status =
    typeof error === "object" &&
    error !== null &&
    "status" in error &&
    typeof (error as { status?: unknown }).status === "number"
      ? (error as { status: number }).status
      : undefined;
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "object" &&
          error !== null &&
          "message" in error &&
          typeof (error as { message?: unknown }).message === "string"
        ? (error as { message: string }).message
        : "";
  if (message.includes("FILE_VERSION_TOO_LARGE"))
    return "filePreviewVersionTooLarge";
  if (status === 403) return "filePreviewNoAccessNow";
  if (status === 404) return "filePreviewVersionReleased";
  if (status === 413) return "filePreviewVersionTooLarge";
  if (status === 409 || status === 503) return "filePreviewPreviewUnavailable";
  return "filePreviewTicketError";
}
