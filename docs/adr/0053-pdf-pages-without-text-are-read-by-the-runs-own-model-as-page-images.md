# PDF pages without text are read by the Run's own model as page images

Decided on 2026-10-15 for #227. Page reading extracts a PDF's text layer within fixed bounds (amended below: in a bounded child process). A page whose text layer is effectively empty (fewer than 16 non-whitespace characters) is neither put through OCR nor transcribed by another model. Instead the Document snapshot keeps the original PDF, and the host research operation `view` renders one such page as an image for the Run's own model, when that model declares image input. The image the model received is an Evidence fragment, cited like a passage, and the evidence viewer shows it. As with `file_read` media, the image is sent once, and later requests carry only the model's own note about it.

## Considered Options

- **OCR on the host.** Tesseract is native code, which would push it into an Execution Pod and make scanned PDFs depend on a runner. The pure-Rust OCR available reads only Latin script. Rejected.
- **A separate vision model transcribes pages into text.** The text would be searchable, but the Evidence fragment would then be model-generated text that can misread or invent, and someone would have to choose and pay for that model. Rejected.
- **Keep every PDF and let the model view any page.** Figures and tables in text PDFs would become readable, but every PDF read would be stored twice. Deferred.

## Consequences

- An original is part of its Document snapshot's contents: the Retention Rules purge it (ADR 0045), Evidence Revocation removes it (ADR 0046), and a Purged snapshot keeps only its digest. A Run keeps at most 64 MiB of originals; pages beyond that are `original_not_retained`.
- A page image is at most 1568 px on its long edge and 1 MiB. One `view` renders one page, and a Run may view 30 pages.
- A page-image fragment keeps the image actually delivered after its snapshot is purged, as passages do, and Evidence Revocation replaces it with a tombstone.
- `find` cannot search page images. A Run whose model has no image input is told which pages carry no text but cannot read them.
- Declared Research providers return text only (`aidash.research/v1`, ADR 0048), so they supply no originals.

## Amendment (2026-10-16, #312 implementation)

- The pure-Rust parser (`hayro`) caps neither decompression nor time, so the server never parses a PDF in its own process. It starts the `aidash` binary again as a child (`aidash __research-pdf extract|render <page>`) with an empty environment, 20 s of CPU time, a 1 GiB address space on Linux, a wall-clock deadline and a bounded reply. Decompression is bounded by that address space instead of per-stream counters.
- `view` is installed with the other research operations; a binding cannot omit one tool per model. A call from a Run whose model definition does not list the `image` modality is refused with `image_input_required` before anything is rendered.
