const REFERENCE =
  "ev_[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
const ANY_MARKER = `\\[\\[cite:${REFERENCE}\\]\\]`;

/**
 * A Source citation marker, `[[cite:ev_<uuid>]]`. It cites something only in
 * text a Run published, and only that Run's own evidence.
 */
const MARKER = new RegExp(`\\[\\[cite:(${REFERENCE})\\]\\]`, "g");

/**
 * Composer inline syntax: bold, italics, inline code and mentions. A marker is
 * one indivisible token, so a delimited span can enclose a citation and the
 * underscore in a reference can neither open nor close italics.
 */
const INLINE = new RegExp(
  `(${ANY_MARKER}|\\*\\*[^*\\n]+\\*\\*|_(?:${ANY_MARKER}|(?!${ANY_MARKER})[^_\\n])+_|\`[^\`\\n]+\`|@[\\p{L}\\p{N}_-]+)`,
  "gu",
);

export type CitationPart =
  | { kind: "text"; text: string }
  | { kind: "citation"; reference: string; ordinal: number };

export type MessagePart =
  | CitationPart
  | { kind: "strong" | "em" | "code"; parts: CitationPart[] }
  | { kind: "mention"; text: string };

/**
 * Splits text a Run published into literal text and Source citations,
 * numbered by first use; a repeated reference keeps its number. Anything that
 * is not a well-formed marker stays literal text. Pass `ordinals` to continue
 * the numbering of earlier text.
 */
export function citationParts(
  text: string,
  ordinals = new Map<string, number>(),
): CitationPart[] {
  const parts: CitationPart[] = [];
  let last = 0;
  for (const match of text.matchAll(MARKER)) {
    if (match.index > last)
      parts.push({ kind: "text", text: text.slice(last, match.index) });
    const reference = match[1].toLowerCase();
    const ordinal = ordinals.get(reference) ?? ordinals.size + 1;
    ordinals.set(reference, ordinal);
    parts.push({ kind: "citation", reference, ordinal });
    last = match.index + match[0].length;
  }
  if (last < text.length) parts.push({ kind: "text", text: text.slice(last) });
  return parts;
}

/**
 * Splits message text into composer inline syntax in one pass. Only in text a
 * Run published (`cited`) do markers become Source citations, numbered by
 * first use across the whole text, including inside bold, italic and code.
 */
export function messageParts(text: string, cited: boolean): MessagePart[] {
  const ordinals = new Map<string, number>();
  const spans = (span: string): CitationPart[] =>
    cited
      ? citationParts(span, ordinals)
      : span
        ? [{ kind: "text", text: span }]
        : [];
  // The single capture group puts every token at an odd index.
  return text.split(INLINE).flatMap((part, index): MessagePart[] => {
    if (index % 2 === 0 || part.startsWith("[[")) return spans(part);
    if (part.startsWith("**"))
      return [{ kind: "strong", parts: spans(part.slice(2, -2)) }];
    if (part.startsWith("_"))
      return [{ kind: "em", parts: spans(part.slice(1, -1)) }];
    if (part.startsWith("`"))
      return [{ kind: "code", parts: spans(part.slice(1, -1)) }];
    return [{ kind: "mention", text: part }];
  });
}

/** Only an absolute http(s) address is offered as a link to the live page. */
export function linkable(url: string): boolean {
  return /^https?:\/\//i.test(url);
}
