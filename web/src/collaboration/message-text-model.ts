/** Composer inline syntax: bold, italics, inline code and mentions. */
const INLINE = /(\*\*[^*\n]+\*\*|_[^_\n]+_|`[^`\n]+`|@[\p{L}\p{N}_-]+)/gu;

export type MessagePart = {
  kind: "text" | "strong" | "em" | "code" | "mention";
  text: string;
};

/**
 * Splits message text into composer inline syntax in one pass. Anything that
 * is not a well-formed span stays literal text.
 */
export function messageParts(text: string): MessagePart[] {
  // The single capture group puts every token at an odd index.
  return text.split(INLINE).flatMap((part, index): MessagePart[] => {
    if (index % 2 === 0) return part ? [{ kind: "text", text: part }] : [];
    if (part.startsWith("**"))
      return [{ kind: "strong", text: part.slice(2, -2) }];
    if (part.startsWith("_")) return [{ kind: "em", text: part.slice(1, -1) }];
    if (part.startsWith("`"))
      return [{ kind: "code", text: part.slice(1, -1) }];
    return [{ kind: "mention", text: part }];
  });
}
