/** An absent bound is unrestricted; an empty bound permits no scopes. */
export function scopeIntersection(
  declared?: readonly string[] | null,
  supported?: readonly string[] | null,
  ceiling?: readonly string[] | null,
): string[] {
  const candidates = declared ?? supported ?? ceiling ?? [];
  return [...new Set(candidates)]
    .filter((scope) =>
      [declared, supported, ceiling].every(
        (bound) => bound == null || bound.includes(scope),
      ),
    )
    .sort();
}

export function canonicalResource(value: string): string | null {
  try {
    if (/\s|[?#]/.test(value) || !value.includes("://")) return null;
    const authority = value.split("://")[1]?.split(/[\\/]/)[0];
    const url = new URL(value);
    if (url.username || url.password || authority?.includes("@")) return null;
    if (
      url.protocol !== "https:" &&
      !(
        url.protocol === "http:" &&
        (/^127\.\d+\.\d+\.\d+$/.test(url.hostname) || url.hostname === "[::1]")
      )
    )
      return null;
    return `${url.origin}${url.pathname.replace(/\/+$/, "")}`;
  } catch {
    return null;
  }
}

export function resourceKey(resource: string): string {
  return btoa(String.fromCharCode(...new TextEncoder().encode(resource)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}
