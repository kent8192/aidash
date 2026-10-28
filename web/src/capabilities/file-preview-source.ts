import {
  FILE_PREVIEW_BYTE_LIMIT,
  type FilePreviewVersion,
  type FileVersionResolver,
} from "../file-preview";
import { resolveWorkingAreaPath } from "../file-preview/working-area-path";
import { download, type CoreFile } from "./client";

/**
 * Resolves Markdown images against the Working Area's current file list.
 *
 * This is deliberately one function so the source can be swapped for a pinned
 * revision manifest when W2/W4 provide one: the pane only depends on the
 * returned resolver, not on how sibling File versions are found.
 */
export function createCurrentManifestResolver(
  area: string,
  markdownPath: string,
  files: readonly CoreFile[],
): FileVersionResolver {
  const cache = new Map<string, Promise<FilePreviewVersion | null>>();
  return (reference) => {
    const path = resolveWorkingAreaPath(markdownPath, reference);
    if (!path) return null;
    const file = files.find((entry) => entry.path === path);
    if (
      !file ||
      !file.media_type.startsWith("image/") ||
      file.size > FILE_PREVIEW_BYTE_LIMIT
    ) {
      return null;
    }
    const pending = cache.get(path);
    if (pending) return pending;
    const loaded = download(
      `/working-areas/${area}/files/${file.file_id}/download`,
    )
      .then(async ({ blob, file: fetched }) => {
        if (fetched.digest !== file.digest) return null;
        if (fetched.size > FILE_PREVIEW_BYTE_LIMIT) return null;
        return {
          bytes: await blob.arrayBuffer(),
          mediaType: fetched.media_type,
          fileName: fetched.path,
        } satisfies FilePreviewVersion;
      })
      .catch(() => null);
    cache.set(path, loaded);
    return loaded;
  };
}
