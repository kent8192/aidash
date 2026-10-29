/**
 * A deliberately small Markdown renderer input, the in-repo replacement for a
 * third-party renderer because web/package.json has none. It supports the
 * subset the dashboard shows for a File version, and it never produces HTML:
 * raw HTML is dropped, links are limited to http/https and images are limited
 * to sibling File versions of the same revision. The React renderer turns
 * these nodes into elements, so there is no `dangerouslySetInnerHTML` path at
 * all. Keep it that way if this is ever swapped for a library.
 */
export type MarkdownInline =
  | { kind: "text"; text: string }
  | { kind: "strong"; text: string }
  | { kind: "em"; text: string }
  | { kind: "code"; text: string }
  | { kind: "link"; href: string; text: string }
  | { kind: "image"; path: string; alt: string }
  | { kind: "image-refused"; alt: string; reason: "not-same-revision" };

export type MarkdownBlock =
  | { kind: "heading"; level: number; children: MarkdownInline[] }
  | { kind: "paragraph"; children: MarkdownInline[] }
  | { kind: "code"; language: string; text: string }
  | { kind: "list"; ordered: boolean; items: MarkdownInline[][] }
  | { kind: "blockquote"; blocks: MarkdownBlock[] }
  | { kind: "rule" };

const fencePattern = /^ {0,3}(`{3,}|~{3,})[ \t]*([^`\n]*)$/;
const headingPattern = /^ {0,3}(#{1,6})[ \t]+(.*?)[ \t]*#*[ \t]*$/;
const rulePattern =
  /^ {0,3}(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$/;
const quotePattern = /^ {0,3}>[ \t]?(.*)$/;
const listPattern = /^ {0,3}([-+*]|\d{1,9}[.)])[ \t]+(.*)$/;

/** Bounds on untrusted Markdown so one File version cannot stall the UI. */
const syntaxWindow = 2048;
const maxInlineDepth = 8;
const maxBlockDepth = 16;
const maxFailedScans = 64;

export function parseMarkdown(source: string): MarkdownBlock[] {
  return parseBlocks(source.replace(/\r\n?/g, "\n").split("\n"));
}

function parseBlocks(lines: string[], depth = 0): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }

    const fence = fencePattern.exec(line);
    if (fence) {
      const marker = fence[1][0];
      const minimum = fence[1].length;
      const closing = new RegExp(
        `^ {0,3}${marker === "`" ? "`" : "~"}{${minimum},}[ \\t]*$`,
      );
      const content: string[] = [];
      index += 1;
      while (index < lines.length && !closing.test(lines[index])) {
        content.push(lines[index]);
        index += 1;
      }
      if (index < lines.length) index += 1;
      blocks.push({
        kind: "code",
        language: fence[2].trim().slice(0, 32),
        text: content.join("\n"),
      });
      continue;
    }

    const heading = headingPattern.exec(line);
    if (heading) {
      blocks.push({
        kind: "heading",
        level: heading[1].length,
        children: parseMarkdownInline(heading[2]),
      });
      index += 1;
      continue;
    }

    if (rulePattern.test(line)) {
      blocks.push({ kind: "rule" });
      index += 1;
      continue;
    }

    if (quotePattern.test(line)) {
      const quoted: string[] = [];
      while (index < lines.length) {
        const match = quotePattern.exec(lines[index]);
        if (!match) break;
        quoted.push(match[1]);
        index += 1;
      }
      if (depth >= maxBlockDepth) {
        blocks.push({
          kind: "paragraph",
          children: parseMarkdownInline(quoted.join(" ")),
        });
      } else {
        blocks.push({
          kind: "blockquote",
          blocks: parseBlocks(quoted, depth + 1),
        });
      }
      continue;
    }

    const list = listPattern.exec(line);
    if (list) {
      const ordered = /^\d/.test(list[1]);
      const items: MarkdownInline[][] = [];
      while (index < lines.length) {
        const match = listPattern.exec(lines[index]);
        if (!match || /^\d/.test(match[1]) !== ordered) break;
        items.push(parseMarkdownInline(match[2]));
        index += 1;
      }
      blocks.push({ kind: "list", ordered, items });
      continue;
    }

    const paragraph: string[] = [];
    while (index < lines.length) {
      const next = lines[index];
      if (!next.trim()) break;
      if (
        fencePattern.test(next) ||
        headingPattern.test(next) ||
        rulePattern.test(next) ||
        quotePattern.test(next) ||
        listPattern.test(next)
      ) {
        break;
      }
      paragraph.push(next.trim());
      index += 1;
    }
    if (paragraph.length > 0) {
      blocks.push({
        kind: "paragraph",
        children: parseMarkdownInline(paragraph.join(" ")),
      });
    }
  }
  return blocks;
}

export function parseMarkdownInline(
  source: string,
  depth = 0,
): MarkdownInline[] {
  if (depth > maxInlineDepth) return [{ kind: "text", text: source }];
  const nodes: MarkdownInline[] = [];
  let buffer = "";
  let failedCode = 0;
  let failedLinks = 0;
  let failedEmphasis = 0;
  let failedTags = 0;
  const flush = () => {
    if (buffer) {
      nodes.push({ kind: "text", text: buffer });
      buffer = "";
    }
  };

  let index = 0;
  while (index < source.length) {
    const char = source[index];

    if (char === "\\" && index + 1 < source.length) {
      buffer += source[index + 1];
      index += 2;
      continue;
    }

    if (char === "`") {
      const run = runLength(source, index, "`");
      if (failedCode >= maxFailedScans) {
        buffer += source.slice(index, index + run);
        index += run;
        continue;
      }
      const base = index + run;
      const window = source.slice(base, base + syntaxWindow);
      const relative = findRun(window, 0, "`", run);
      if (relative >= 0) {
        flush();
        nodes.push({
          kind: "code",
          text: window.slice(0, relative).replace(/\n/g, " "),
        });
        index = base + relative + run;
        continue;
      }
      failedCode += 1;
      buffer += source.slice(index, index + run);
      index += run;
      continue;
    }

    if (
      char === "!" &&
      source[index + 1] === "[" &&
      failedLinks < maxFailedScans
    ) {
      const window = source.slice(index, index + syntaxWindow);
      const image = /^!\[([^\]]*)\]\(\s*([^\s)]+)(?:\s+"[^"]*")?\s*\)/.exec(
        window,
      );
      if (image) {
        flush();
        const alt = plainInline(image[1], depth);
        const path = sameRevisionImagePath(image[2]);
        nodes.push(
          path
            ? { kind: "image", path, alt }
            : { kind: "image-refused", alt, reason: "not-same-revision" },
        );
        index += image[0].length;
        continue;
      }
      failedLinks += 1;
    }

    if (char === "[" && failedLinks < maxFailedScans) {
      const window = source.slice(index, index + syntaxWindow);
      const link = /^\[([^\]]*)\]\(\s*([^\s)]+)(?:\s+"[^"]*")?\s*\)/.exec(
        window,
      );
      if (link) {
        flush();
        const text = plainInline(link[1], depth);
        if (isHttpSafeUrl(link[2])) {
          nodes.push({ kind: "link", href: link[2].trim(), text });
        } else {
          nodes.push({ kind: "text", text });
        }
        index += link[0].length;
        continue;
      }
      failedLinks += 1;
    }

    if (
      (source.startsWith("**", index) || source.startsWith("__", index)) &&
      failedEmphasis < maxFailedScans
    ) {
      const marker = source.slice(index, index + 2);
      const closing = source
        .slice(index + 2, index + 2 + syntaxWindow)
        .indexOf(marker);
      if (closing > 0) {
        flush();
        nodes.push({
          kind: "strong",
          text: plainInline(
            source.slice(index + 2, index + 2 + closing),
            depth,
          ),
        });
        index += 2 + closing + 2;
        continue;
      }
      failedEmphasis += 1;
    }

    if (char === "*" || char === "_") {
      if (char === "_" && isWordCharacter(source[index - 1])) {
        buffer += char;
        index += 1;
        continue;
      }
      if (
        source[index + 1] &&
        !/\s/.test(source[index + 1]) &&
        failedEmphasis < maxFailedScans
      ) {
        const base = index + 1;
        const window = source.slice(base, base + syntaxWindow);
        const closing = findEmphasisEnd(window, 0, char);
        if (closing > 0) {
          flush();
          nodes.push({
            kind: "em",
            text: plainInline(window.slice(0, closing), depth),
          });
          index = base + closing + 1;
          continue;
        }
        failedEmphasis += 1;
      }
    }

    if (char === "<" && failedTags < maxFailedScans) {
      if (source.startsWith("<!--", index)) {
        const end = source.indexOf("-->", index + 4);
        index = end >= 0 ? end + 3 : source.length;
        continue;
      }
      const window = source.slice(index, index + syntaxWindow);
      const autolink = /^<([a-zA-Z][a-zA-Z0-9+.-]*:[^<>\s]+)>/.exec(window);
      if (autolink) {
        flush();
        if (isHttpSafeUrl(autolink[1])) {
          nodes.push({ kind: "link", href: autolink[1], text: autolink[1] });
        } else {
          nodes.push({ kind: "text", text: autolink[1] });
        }
        index += autolink[0].length;
        continue;
      }
      let end = -1;
      const limit = Math.min(window.length, syntaxWindow);
      for (let next = 1; next < limit; next += 1) {
        if (window[next] === ">") {
          end = next;
          break;
        }
      }
      if (end > 0) {
        // Raw HTML is never emitted; the tag itself disappears.
        index += end + 1;
        continue;
      }
      failedTags += 1;
    }

    buffer += char;
    index += 1;
  }
  flush();
  return nodes;
}

/** Only absolute http/https URLs may become links. */
export function isHttpSafeUrl(value: string): boolean {
  const trimmed = value.trim();
  if (!/^https?:\/\//i.test(trimmed)) return false;
  if (/\s/.test(trimmed) || hasControlCharacter(trimmed)) return false;
  try {
    const parsed = new URL(trimmed);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * Returns the path handed to `FileVersionResolver`, or null for anything
 * that is not a same-revision reference: schemes, protocol-relative and
 * absolute paths, fragments and control characters. `../` is left in place;
 * the resolver is the authority on what the revision contains.
 */
export function sameRevisionImagePath(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(trimmed)) return null;
  if (trimmed.startsWith("//")) return null;
  if (trimmed.startsWith("/") || trimmed.startsWith("\\")) return null;
  if (trimmed.startsWith("#")) return null;
  if (trimmed.includes("\\") || hasControlCharacter(trimmed)) {
    return null;
  }
  const query = trimmed.indexOf("?");
  const fragment = trimmed.indexOf("#");
  const end =
    query < 0 ? fragment : fragment < 0 ? query : Math.min(query, fragment);
  const path = end < 0 ? trimmed : trimmed.slice(0, end);
  return path || null;
}

function hasControlCharacter(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

function runLength(source: string, start: number, char: string): number {
  let length = 0;
  while (source[start + length] === char) length += 1;
  return length;
}

function findRun(
  source: string,
  start: number,
  char: string,
  length: number,
): number {
  let index = start;
  while (index < source.length) {
    const candidate = source.indexOf(char, index);
    if (candidate < 0) return -1;
    if (runLength(source, candidate, char) === length) return candidate;
    index = candidate + 1;
  }
  return -1;
}

function findEmphasisEnd(source: string, start: number, char: string): number {
  for (let index = start; index < source.length; index += 1) {
    if (source[index] !== char) continue;
    if (index === 0 || /\s/.test(source[index - 1])) continue;
    if (char === "_" && isWordCharacter(source[index + 1])) continue;
    return index;
  }
  return -1;
}

function isWordCharacter(char: string | undefined): boolean {
  return !!char && /[A-Za-z0-9_]/.test(char);
}

function plainInline(source: string, depth: number): string {
  return flattenInline(parseMarkdownInline(source, depth + 1));
}

function flattenInline(nodes: MarkdownInline[]): string {
  return nodes
    .map((node) => {
      switch (node.kind) {
        case "text":
        case "strong":
        case "em":
        case "code":
          return node.text;
        case "link":
          return node.text;
        case "image":
        case "image-refused":
          return node.alt;
      }
    })
    .join("");
}
