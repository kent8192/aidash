/** The server accepts at most eight cited File versions per message. */
export const MAX_FILE_CITATIONS = 8;

/** One File version waiting in a composer as a citation chip. */
export interface FileCitation {
  key: string;
  areaId: string;
  fileId: string;
  path: string;
  digest?: string;
  size?: number;
  mediaType?: string;
  revision?: number;
}

export function fileCitationKey(areaId: string, fileId: string): string {
  return `${areaId}:${fileId}`;
}

export function fileCitationFrom(
  areaId: string,
  file: {
    file_id: string;
    path: string;
    digest?: string;
    size?: number;
    media_type?: string;
  },
  revision?: number,
): FileCitation {
  return {
    key: fileCitationKey(areaId, file.file_id),
    areaId,
    fileId: file.file_id,
    path: file.path,
    ...(file.digest ? { digest: file.digest } : {}),
    ...(file.size !== undefined ? { size: file.size } : {}),
    ...(file.media_type ? { mediaType: file.media_type } : {}),
    ...(revision !== undefined ? { revision } : {}),
  };
}

/** Idempotent add; duplicates and a ninth citation leave the list unchanged. */
export function addFileCitation(
  citations: readonly FileCitation[],
  citation: FileCitation,
): readonly FileCitation[] {
  if (citations.length >= MAX_FILE_CITATIONS) return citations;
  if (citations.some((entry) => entry.key === citation.key)) return citations;
  return [...citations, citation];
}

export function removeFileCitation(
  citations: readonly FileCitation[],
  key: string,
): readonly FileCitation[] {
  return citations.filter((entry) => entry.key !== key);
}

/** The request field for a message that cites these exact File versions. */
export function fileCitationPayload(
  citations: readonly FileCitation[],
): { area_id: string; file_id: string }[] {
  return citations.map((citation) => ({
    area_id: citation.areaId,
    file_id: citation.fileId,
  }));
}
