/**
 * Resolves a Markdown image reference against the file that contains it. The
 * caller supplies a path relative to the Working Area revision (no leading
 * slash), or null when the reference would leave that revision.
 */
export function resolveWorkingAreaPath(
  fromPath: string,
  reference: string,
): string | null {
  const trimmed = reference.trim();
  if (!trimmed) return null;
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(trimmed)) return null;
  if (trimmed.startsWith("//") || trimmed.startsWith("/")) return null;
  if (trimmed.startsWith("#")) return null;
  if (trimmed.includes("\\") || hasControlCharacter(trimmed)) return null;
  const query = trimmed.indexOf("?");
  const fragment = trimmed.indexOf("#");
  const end =
    query < 0 ? fragment : fragment < 0 ? query : Math.min(query, fragment);
  const relative = end < 0 ? trimmed : trimmed.slice(0, end);
  if (!relative) return null;

  const segments = fromPath.split("/").filter(Boolean).slice(0, -1);
  for (const segment of relative.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      if (segments.length === 0) return null;
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  return segments.length > 0 ? segments.join("/") : null;
}

function hasControlCharacter(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}
