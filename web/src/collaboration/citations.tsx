import { Fragment, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ExternalLink } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/popover";
import { Alert, Loading } from "../components/patterns";
import { runEvidence } from "../generated/aidash";
import type { CitedEvidence } from "../generated/models";
import { ApiError } from "../transport";
import { useI18n } from "../ui";
import { citationCopy } from "./citation-copy";
import { linkable, type CitationPart } from "./citations-model";

/**
 * Literal text with Source citations as numbered chips. Each chip reads its
 * evidence only when opened, and only from the publishing Run.
 */
export function CitedText({
  run,
  parts,
}: {
  run?: string | null;
  parts: readonly CitationPart[];
}) {
  return (
    <>
      {parts.map((part, index) =>
        part.kind === "text" ? (
          <Fragment key={index}>{part.text}</Fragment>
        ) : (
          // Parts hold citations only when parsed for the publishing Run.
          run && (
            <Citation
              key={index}
              run={run}
              reference={part.reference}
              ordinal={part.ordinal}
            />
          )
        ),
      )}
    </>
  );
}

function Citation({
  run,
  reference,
  ordinal,
}: {
  run: string;
  reference: string;
  ordinal: number;
}) {
  const { locale } = useI18n();
  const copy = citationCopy[locale];
  // Each opening is a separate read, so access lost since the last one is
  // enforced and evidence from an earlier opening is never shown again.
  const [opening, setOpening] = useState(0);
  return (
    <Popover onOpenChange={(open) => open && setOpening((count) => count + 1)}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="mx-0.5 inline-flex h-4 min-w-4 items-center justify-center rounded-sm border border-brand-line px-1 align-[0.1em] font-mono text-[10px] leading-none text-brand transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
          aria-label={copy.citation(ordinal)}
          data-citation={reference}
        >
          {ordinal}
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="grid max-h-[70vh] w-[min(28rem,calc(100vw-2rem))] gap-2 overflow-auto"
        aria-label={copy.citation(ordinal)}
      >
        <CitationRead run={run} reference={reference} opening={opening} />
      </PopoverContent>
    </Popover>
  );
}

/** Mounted only while the citation is open; nothing it read outlives it. */
function CitationRead({
  run,
  reference,
  opening,
}: {
  run: string;
  reference: string;
  opening: number;
}) {
  const { locale } = useI18n();
  const copy = citationCopy[locale];
  const evidence = useQuery({
    queryKey: ["run-evidence", run, reference, opening],
    queryFn: () => runEvidence(run, reference),
    retry: false,
    gcTime: 0,
  });
  // Every 404 is the same absence: no permission, no such evidence, or gone.
  const unavailable =
    evidence.error instanceof ApiError && evidence.error.status === 404;
  return (
    <>
      {evidence.isPending && <Loading>{copy.loading}</Loading>}
      {unavailable && (
        <p role="status" className="text-xs text-muted-foreground">
          {copy.unavailable}
        </p>
      )}
      {evidence.isError && !unavailable && (
        <Alert retry={() => void evidence.refetch()}>{copy.failed}</Alert>
      )}
      {/* A failed refetch keeps the previous data; never show it then. */}
      {evidence.isSuccess && <Evidence evidence={evidence.data} />}
    </>
  );
}

function Evidence({ evidence }: { evidence: CitedEvidence }) {
  const { locale } = useI18n();
  const copy = citationCopy[locale];
  const title = evidence.title?.trim() || evidence.url;
  return (
    <article className="grid min-w-0 gap-2 text-xs">
      <header className="grid min-w-0 gap-0.5">
        {linkable(evidence.url) ? (
          <a
            href={evidence.url}
            target="_blank"
            rel="noopener noreferrer nofollow"
            className="inline-flex min-w-0 items-center gap-1 font-medium text-foreground hover:underline"
          >
            <span className="truncate">{title}</span>
            <ExternalLink aria-hidden className="size-3 shrink-0" />
          </a>
        ) : (
          <strong className="truncate font-medium text-foreground">
            {title}
          </strong>
        )}
        <span className="truncate font-mono text-[11px] text-faint">
          {evidence.url}
        </span>
        {evidence.requested_url !== evidence.url && (
          <span className="truncate text-[11px] text-faint">
            {copy.requested}:{" "}
            <span className="font-mono">{evidence.requested_url}</span>
          </span>
        )}
        <span className="text-[11px] text-faint">
          {copy.fetched}:{" "}
          <time dateTime={evidence.fetched_at} className="font-mono tabular">
            {new Date(evidence.fetched_at).toLocaleString(locale)}
          </time>
          {evidence.pages.length > 0 && (
            <>
              {" · "}
              {copy.pages}: {evidence.pages.join(", ")}
            </>
          )}
        </span>
      </header>
      {evidence.image ? (
        // A viewed page: exactly the image the agent received. It has no text
        // and no surrounding lines.
        <section className="grid gap-1" aria-label={copy.pageImage}>
          <h4 className="text-[11px] font-medium text-faint">
            {copy.pageImage}
          </h4>
          <img
            src={`data:${evidence.image.media_type};base64,${evidence.image.data}`}
            width={evidence.image.width}
            height={evidence.image.height}
            alt={copy.pageImageAlt(evidence.image.page)}
            className="h-auto max-w-full border border-border"
          />
        </section>
      ) : (
        <PassageWithContext evidence={evidence} />
      )}
    </article>
  );
}

function PassageWithContext({ evidence }: { evidence: CitedEvidence }) {
  const { locale } = useI18n();
  const copy = citationCopy[locale];
  return (
    <>
      <section className="grid gap-1" aria-label={copy.excerpt}>
        <h4 className="text-[11px] font-medium text-faint">{copy.excerpt}</h4>
        <blockquote className="whitespace-pre-wrap border-l-2 border-brand-line pl-2 leading-relaxed text-foreground [overflow-wrap:anywhere]">
          {evidence.text}
        </blockquote>
        {evidence.completeness === "text_limit" && (
          <p className="text-[11px] text-faint">{copy.partial}</p>
        )}
      </section>
      {evidence.context ? (
        (evidence.context.before.length > 0 ||
          evidence.context.after.length > 0) && (
          <details className="group grid gap-1">
            <summary className="cursor-pointer text-[11px] font-medium text-faint">
              {copy.context}
            </summary>
            <div className="mt-1 grid gap-1 whitespace-pre-wrap font-mono text-[11px] leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
              {evidence.context.before.map((line, index) => (
                <p key={`before-${index}`}>{line.text}</p>
              ))}
              <p className="border-l-2 border-brand-line pl-2 text-foreground">
                {evidence.text}
              </p>
              {evidence.context.after.map((line, index) => (
                <p key={`after-${index}`}>{line.text}</p>
              ))}
            </div>
          </details>
        )
      ) : (
        <p className="text-[11px] text-faint">{copy.noContext}</p>
      )}
    </>
  );
}
