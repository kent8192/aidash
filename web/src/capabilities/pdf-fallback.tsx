import { useEffect, useRef, useState } from "react";
import { Alert, Loading } from "../components/patterns";
import { Button } from "../components/ui/button";
import { useI18n } from "../ui";
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

export function PdfFallback({ bytes }: { bytes: ArrayBuffer }) {
  const { t } = useI18n();
  const canvas = useRef<HTMLCanvasElement>(null);
  const [page, setPage] = useState(1);
  const [pages, setPages] = useState(0);
  const [error, setError] = useState(false);
  useEffect(() => {
    let cancelled = false;
    let destroy: (() => Promise<void>) | undefined;
    void (async () => {
      try {
        const pdfjs = await import("pdfjs-dist");
        if (cancelled) return;
        pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;
        const options = {
          data: bytes.slice(0),
          isEvalSupported: false,
          enableScripting: false,
          enableXfa: false,
        };
        const task = pdfjs.getDocument(options);
        destroy = () => task.destroy();
        const pdf = await task.promise;
        if (cancelled) return;
        setPages(pdf.numPages);
        const pdfPage = await pdf.getPage(page);
        if (cancelled || !canvas.current) return;
        const natural = pdfPage.getViewport({ scale: 1 });
        const scale = Math.min(1, 2048 / natural.width, 2048 / natural.height);
        const viewport = pdfPage.getViewport({ scale });
        const target = canvas.current;
        target.width = viewport.width;
        target.height = viewport.height;
        await pdfPage.render({
          canvas: target,
          viewport,
          annotationMode: pdfjs.AnnotationMode.DISABLE,
        }).promise;
      } catch {
        if (!cancelled) setError(true);
      }
    })();
    return () => {
      cancelled = true;
      void destroy?.();
    };
  }, [bytes, page]);
  if (error) return <Alert>{t("filePreviewPaneFetchError")}</Alert>;
  return (
    <div className="grid gap-2">
      {!pages && <Loading>{t("filePreviewPaneLoading")}</Loading>}
      <canvas key={page} ref={canvas} className="max-w-full" />
      {pages > 1 && (
        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={page <= 1}
            onClick={() => setPage(page - 1)}
          >
            {t("filePreviewPdfPrevious")}
          </Button>
          <span>
            {page} / {pages}
          </span>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={page >= pages}
            onClick={() => setPage(page + 1)}
          >
            {t("filePreviewPdfNext")}
          </Button>
        </div>
      )}
    </div>
  );
}
