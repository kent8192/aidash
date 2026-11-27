import { cn } from "../lib/utils";
import { useEffect, useState } from "react";
import { ImageFilePreview } from "./image-preview.tsx";
import { exceedsFilePreviewLimit } from "./limits.ts";
import { MarkdownFilePreview } from "./markdown-preview.tsx";
import { classifyFilePreview, videoMediaType } from "./preview-kind.ts";
import { SpreadsheetFilePreview } from "./spreadsheet-preview.tsx";
import {
  FilePreviewLimitState,
  FilePreviewPreviewDomainState,
  FilePreviewUnsupportedState,
} from "./states.tsx";
import type { FilePreviewVersion, FileVersionResolver } from "./types.ts";

export interface FilePreviewRendererProps extends FilePreviewVersion {
  /**
   * Resolves Markdown images to sibling File versions of the same revision.
   * Without it, Markdown images show their alt text with a note.
   */
  resolveRelativePath?: FileVersionResolver;
  /**
   * Opens an HTML or PDF File version on the preview domain. The dashboard
   * never renders those formats itself.
   */
  onOpenPreviewDomain?: (version: FilePreviewVersion) => void;
  onDownload?: (version: FilePreviewVersion) => void;
  className?: string;
}

/**
 * The one dispatcher for dashboard File previews. Every branch either renders
 * a File preview or explains why this File version has none here; it never
 * falls back to showing bytes as text.
 */
export function FilePreviewRenderer({
  bytes,
  mediaType,
  fileName,
  resolveRelativePath,
  onOpenPreviewDomain,
  onDownload,
  className,
}: FilePreviewRendererProps) {
  const version: FilePreviewVersion = { bytes, mediaType, fileName };
  const kind = classifyFilePreview(fileName, mediaType);

  switch (kind) {
    case "video":
      return exceedsFilePreviewLimit(bytes) ? (
        <FilePreviewLimitState version={version} onDownload={onDownload} />
      ) : (
        <VideoFilePreview {...version} className={className} />
      );
    case "image":
    case "svg":
      if (exceedsFilePreviewLimit(bytes)) {
        return (
          <div className={cn("min-w-0", className)}>
            <FilePreviewLimitState version={version} onDownload={onDownload} />
          </div>
        );
      }
      return (
        <ImageFilePreview
          bytes={bytes}
          mediaType={mediaType}
          fileName={fileName}
          className={className}
          onDownload={onDownload}
        />
      );
    case "markdown":
      if (exceedsFilePreviewLimit(bytes)) {
        return (
          <div className={cn("min-w-0", className)}>
            <FilePreviewLimitState version={version} onDownload={onDownload} />
          </div>
        );
      }
      return (
        <MarkdownFilePreview
          bytes={bytes}
          mediaType={mediaType}
          fileName={fileName}
          resolveRelativePath={resolveRelativePath}
          className={className}
          onDownload={onDownload}
        />
      );
    case "csv":
    case "xlsx":
      return (
        <SpreadsheetFilePreview
          kind={kind}
          bytes={bytes}
          mediaType={mediaType}
          fileName={fileName}
          className={className}
          onDownload={onDownload}
        />
      );
    case "preview-domain":
      return (
        <div className={cn("min-w-0", className)}>
          <FilePreviewPreviewDomainState
            version={version}
            onOpenPreviewDomain={onOpenPreviewDomain}
          />
        </div>
      );
    case "unsupported":
      return (
        <div className={cn("min-w-0", className)}>
          <FilePreviewUnsupportedState
            version={version}
            onDownload={onDownload}
          />
        </div>
      );
  }
}

function VideoFilePreview({
  bytes,
  mediaType,
  fileName,
  className,
}: FilePreviewVersion & { className?: string }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    let live = true;
    let src: string | undefined;
    void Promise.resolve().then(() => {
      if (!live) return;
      src = URL.createObjectURL(
        new Blob([bytes], { type: videoMediaType(fileName, mediaType) }),
      );
      setUrl(src);
    });
    return () => {
      live = false;
      if (src) URL.revokeObjectURL(src);
    };
  }, [bytes, fileName, mediaType]);
  if (!url) return null;
  // File previews play the user's exact bytes, including any embedded caption tracks.
  return (
    // eslint-disable-next-line jsx-a11y/media-has-caption
    <video
      src={url}
      aria-label={fileName}
      controls
      playsInline
      preload="metadata"
      className={cn("max-h-[70vh] w-full rounded-md object-contain", className)}
    />
  );
}
