import { Button } from "../components/ui/button";
import { useFilePreview } from "../capabilities/file-preview-pane";
import { useI18n } from "../ui";

export type ArtifactFileReference = {
  area_id: string;
  file_id: string;
  path: string;
  digest: string;
  size: number;
  media_type: string;
};

export function artifactFile(content: unknown): ArtifactFileReference | null {
  if (typeof content !== "object" || content === null) return null;
  const file = content as Partial<ArtifactFileReference>;
  const id = /^[\da-f]{8}-(?:[\da-f]{4}-){3}[\da-f]{12}$/i;
  if (
    typeof file.area_id !== "string" ||
    !id.test(file.area_id) ||
    typeof file.file_id !== "string" ||
    !id.test(file.file_id) ||
    typeof file.path !== "string" ||
    typeof file.digest !== "string" ||
    typeof file.size !== "number" ||
    !Number.isSafeInteger(file.size) ||
    file.size < 0 ||
    typeof file.media_type !== "string"
  )
    return null;
  return file as ArtifactFileReference;
}

export function ArtifactFile({ file }: { file: ArtifactFileReference }) {
  const { t } = useI18n();
  const preview = useFilePreview();
  return (
    <Button
      variant="outline"
      type="button"
      aria-label={`${t("filePreviewAction")}: ${file.path}`}
      onClick={() =>
        preview.open({
          area: file.area_id,
          file: { ...file, scope: "working" },
          cited: true,
        })
      }
    >
      {file.path}
    </Button>
  );
}
