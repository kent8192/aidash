/**
 * A Source citation marker, `[[cite:ev_<uuid>]]`. It cites something only in
 * text a Run published, and only that Run's own evidence.
 */
const MARKER =
  /\[\[cite:(ev_[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\]\]/g;

export type CitationPart =
  | { kind: "text"; text: string }
  | { kind: "citation"; reference: string; ordinal: number };

/**
 * Splits text a Run published into literal text and Source citations,
 * numbered by first use; a repeated reference keeps its number. Anything that
 * is not a well-formed marker stays literal text.
 */
export function citationParts(text: string): CitationPart[] {
  const parts: CitationPart[] = [];
  const ordinals = new Map<string, number>();
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

/** Only an absolute http(s) address is offered as a link to the live page. */
export function linkable(url: string): boolean {
  return /^https?:\/\//i.test(url);
}
