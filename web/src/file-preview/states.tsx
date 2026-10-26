import { Button } from "../components/ui/button";
import { Notice } from "../components/patterns";
import { useI18n } from "../ui";
import type { FilePreviewVersion } from "./types.ts";

export interface FilePreviewStateProps {
  version: FilePreviewVersion;
  onDownload?: (version: FilePreviewVersion) => void;
}

export function FilePreviewLimitState({
  version,
  onDownload,
}: FilePreviewStateProps) {
  const { t } = useI18n();
  return (
    <div className="grid min-w-0 gap-2">
      <Notice tone="warning">{t("filePreviewTooLargeBody")}</Notice>
      {onDownload && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="w-fit"
          onClick={() => onDownload(version)}
        >
          {t("filePreviewDownload")}
        </Button>
      )}
    </div>
  );
}

export function FilePreviewUnsupportedState({
  version,
  onDownload,
}: FilePreviewStateProps) {
  const { t } = useI18n();
  return (
    <div className="grid min-w-0 gap-2">
      <Notice>{t("filePreviewUnsupportedBody")}</Notice>
      {onDownload && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="w-fit"
          onClick={() => onDownload(version)}
        >
          {t("filePreviewDownload")}
        </Button>
      )}
    </div>
  );
}

export function FilePreviewPreviewDomainState({
  version,
  onOpenPreviewDomain,
}: {
  version: FilePreviewVersion;
  onOpenPreviewDomain?: (version: FilePreviewVersion) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="grid min-w-0 gap-2">
      <Notice>{t("filePreviewPreviewDomainBody")}</Notice>
      {onOpenPreviewDomain ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="w-fit"
          onClick={() => onOpenPreviewDomain(version)}
        >
          {t("filePreviewOpenPreviewDomain")}
        </Button>
      ) : (
        <p className="text-xs leading-relaxed text-muted-foreground">
          {t("filePreviewPreviewDomainUnavailable")}
        </p>
      )}
    </div>
  );
}
