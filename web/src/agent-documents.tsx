import { useRef, useState } from "react";
import { Field, useI18n } from "./ui";
import type { ReferenceDocument } from "./generated/models";

const maxText = 65536;
export async function extractDocument(file: File): Promise<ReferenceDocument> {
  if (file.size > 10 * 1024 * 1024) throw new Error("documentFileLimit");
  let text = "";
  let media_type = "text/plain";
  const append = (value: string) => {
    text += value;
    if (new TextEncoder().encode(text).length > maxText)
      throw new Error("documentTextLimit");
  };
  if (/\.pdf$/i.test(file.name)) {
    media_type = "application/pdf";
    const pdfjs = await import("pdfjs-dist");
    pdfjs.GlobalWorkerOptions.workerSrc = new URL(
      "pdfjs-dist/build/pdf.worker.min.mjs",
      import.meta.url,
    ).href;
    const task = pdfjs.getDocument({
      data: await file.arrayBuffer(),
    });
    try {
      const pdf = await task.promise;
      if (pdf.numPages > 200) throw new Error("documentFileLimit");
      for (let n = 1; n <= pdf.numPages; n++) {
        const page = await pdf.getPage(n);
        const content = await page.getTextContent();
        const pageText = content.items
          .map((item) => ("str" in item ? item.str : ""))
          .join(" ");
        if (pageText.trim()) append(`[Page ${n}]\n${pageText}\n`);
      }
    } finally {
      await task.destroy();
    }
  } else if (/\.xlsx$/i.test(file.name)) {
    media_type =
      "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    const { default: ExcelJS } = await import("exceljs");
    const workbook = new ExcelJS.Workbook();
    await workbook.xlsx.load(await file.arrayBuffer());
    workbook.eachSheet((sheet) => {
      let started = false;
      sheet.eachRow((row) => {
        const cells: string[] = [];
        row.eachCell((cell) => {
          if (cell.text.trim()) cells.push(`${cell.address}: ${cell.text}`);
        });
        if (cells.length) {
          if (!started) {
            append(`[Sheet: ${sheet.name}]\n`);
            started = true;
          }
          append(`${cells.join("\t")}\n`);
        }
      });
    });
  } else if (/\.(txt|md|csv)$/i.test(file.name)) {
    append(await file.text());
  } else throw new Error("documentUnsupported");
  if (!text.trim()) throw new Error("documentEmpty");
  return { name: file.name, media_type, text };
}

export function AgentDocuments({
  documents,
  change,
  busyChange,
}: {
  documents: ReferenceDocument[];
  change: (documents: ReferenceDocument[]) => void;
  busyChange: (busy: boolean) => void;
}) {
  const { t } = useI18n();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const sequence = useRef(0);
  return (
    <fieldset>
      <legend>{t("agentDocuments")}</legend>
      <p className="muted">{t("agentDocumentsHelp")}</p>
      <Field label={t("documentUpload")}>
        <input
          type="file"
          accept=".pdf,.xlsx,.txt,.md,.csv"
          multiple
          disabled={busy}
          onChange={async (event) => {
            const files = Array.from(event.target.files ?? []);
            event.target.value = "";
            const current = ++sequence.current;
            setBusy(true);
            busyChange(true);
            setError("");
            try {
              if (documents.length + files.length > 8)
                throw new Error("documentCountLimit");
              const next = [...documents];
              for (const file of files) next.push(await extractDocument(file));
              if (
                new TextEncoder().encode(next.map((d) => d.text).join(""))
                  .length > maxText
              )
                throw new Error("documentTextLimit");
              if (current === sequence.current) change(next);
            } catch (error) {
              setError(
                t(
                  error instanceof Error && error.message.startsWith("document")
                    ? error.message
                    : "documentReadError",
                ),
              );
            } finally {
              setBusy(false);
              busyChange(false);
            }
          }}
        />
      </Field>
      {busy && <p role="status">{t("documentReading")}</p>}
      {error && <p role="alert">{error}</p>}
      {documents.map((document, index) => (
        <div key={`${index}-${document.name}`}>
          <details>
            <summary>{document.name}</summary>
            <pre
              style={{
                whiteSpace: "pre-wrap",
                maxHeight: 200,
                overflow: "auto",
              }}
            >
              {document.text}
            </pre>
          </details>
          <button
            type="button"
            disabled={busy}
            onClick={() => change(documents.filter((_, i) => i !== index))}
          >
            {t("remove")} · {document.name}
          </button>
        </div>
      ))}
    </fieldset>
  );
}
