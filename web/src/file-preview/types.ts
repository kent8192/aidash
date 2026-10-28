/**
 * One exact File version's bytes, as the caller read them for a File preview.
 * The dashboard never asks the Preview gateway for these bytes; the caller
 * already holds the authorized File version.
 */
export interface FilePreviewVersion {
  readonly bytes: ArrayBuffer;
  readonly mediaType: string;
  readonly fileName: string;
}

/**
 * Resolves a Markdown image reference to a sibling File version of the same
 * Working Area revision. The reference is the path as written in the
 * Markdown, with surrounding whitespace removed; the callback must return
 * only a File version of the same revision and `null` for anything it cannot
 * resolve. Returning a File version from another revision would show a
 * different version than the one the person opened.
 */
export type FileVersionResolver = (
  relativePath: string,
) => FilePreviewVersion | null | Promise<FilePreviewVersion | null>;
