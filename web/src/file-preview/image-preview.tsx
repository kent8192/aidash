import { useEffect, useState } from "react";
import { Loading } from "../components/patterns";
import { cn } from "../lib/utils";
import { useI18n } from "../ui";
import { imageMediaType } from "./image.ts";
import { exceedsFilePreviewLimit } from "./limits.ts";
import { FilePreviewLimitState } from "./states.tsx";
import type { FilePreviewVersion } from "./types.ts";

export interface ImageFilePreviewProps extends FilePreviewVersion {
  className?: string;
  onDownload?: (version: FilePreviewVersion) => void;
}

type ImageState = {
  bytes: ArrayBuffer;
  mediaType: string;
  fileName: string;
  url: string;
};

/**
 * png/jpeg/gif/webp and SVG File previews. Both use a blob: object URL in an
 * `<img>`; SVG markup is never inserted into the dashboard's DOM. Every
 * object URL is revoked when the component unmounts or the bytes change.
 */
export function ImageFilePreview({
  bytes,
  mediaType,
  fileName,
  className,
  onDownload,
}: ImageFilePreviewProps) {
  const { t } = useI18n();
  const tooLarge = exceedsFilePreviewLimit(bytes);
  const [state, setState] = useState<ImageState | null>(null);

  useEffect(() => {
    if (tooLarge) return;
    let cancelled = false;
    let objectUrl: string | undefined;
    void Promise.resolve().then(() => {
      if (cancelled) return;
      objectUrl = URL.createObjectURL(
        new Blob([bytes], { type: imageMediaType(mediaType, fileName) }),
      );
      setState({ bytes, mediaType, fileName, url: objectUrl });
    });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [bytes, mediaType, fileName, tooLarge]);

  if (tooLarge) {
    return (
      <FilePreviewLimitState
        version={{ bytes, mediaType, fileName }}
        onDownload={onDownload}
      />
    );
  }
  const current =
    state &&
    state.bytes === bytes &&
    state.mediaType === mediaType &&
    state.fileName === fileName
      ? state
      : null;
  if (!current) return <Loading />;
  return (
    <div className={cn("grid min-w-0 gap-2", className)}>
      <img
        src={current.url}
        alt={fileName || t("filePreviewTitle")}
        className="max-h-[70vh] w-auto max-w-full rounded-md border border-border bg-background object-contain"
      />
    </div>
  );
}
