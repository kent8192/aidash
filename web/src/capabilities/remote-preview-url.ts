/** Accept only H's one-time ticket URL on the domain V got from H. */
export function remotePreviewUrl(ticket: {
  url: string;
  preview_domain: string;
}): string {
  const domain = ticket.preview_domain.trim().replace(/\.$/, "").toLowerCase();
  if (!domain || !/^[a-z0-9.-]+$/.test(domain))
    throw new Error("Invalid remote preview domain");
  const url = new URL(ticket.url);
  const host = url.hostname.toLowerCase();
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    (host !== domain && !host.endsWith(`.${domain}`)) ||
    url.pathname !== "/__aidash/ticket" ||
    !url.searchParams.has("t") ||
    url.hash
  )
    throw new Error("Invalid remote preview ticket");
  return url.href;
}
