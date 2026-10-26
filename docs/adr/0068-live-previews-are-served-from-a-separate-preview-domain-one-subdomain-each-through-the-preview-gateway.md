# Live previews are served from a separate preview domain, one subdomain each, through the Preview gateway

Decided on 2026-10-19 while grilling Live previews. A Live preview runs code an agent wrote, so its pages are untrusted content in a person's browser. Each Live preview, and each File preview that needs the browser's own engine (HTML and PDF), is served from its own random subdomain of a preview domain: a registrable domain separate from the dashboard's and listed on the Public Suffix List, so that no two previews are the same site and none shares cookies, storage or same-site status with the dashboard. A person enters through the Preview gateway with a one-time ticket the dashboard issues after authorization; the gateway exchanges it for a host-only cookie on that subdomain. Under Federation, the person's Deployment asks the hosting Node for a ticket and the browser goes directly to the hosting Deployment's preview domain.

## Considered Options

- **The dashboard's origin, with a sandboxed iframe and a CSP `sandbox` header.** The opaque origin breaks storage, cookies, service workers and credentialed requests, which most development servers rely on, and the gateway would sit beside the session cookie. Rejected.
- **A subdomain of the dashboard's domain.** Previews would be same-site with the dashboard, so they could toss cookies and pass `SameSite=Lax` checks. Rejected.
- **One preview origin routed by path.** Previews of different Tenants would share storage. Rejected.
- **Relaying Federation viewers' traffic between Deployments.** Every request and WebSocket would cross two Deployments. Rejected; the hosting Deployment sees the viewer's address instead.

## Consequences

- A Deployment without a preview domain offers no Live previews and no HTML or PDF File previews. Self-hosted Deployments configure one; a local cluster uses `*.localhost`.
- Cloud needs its own domain, wildcard DNS and certificate, and a Public Suffix List entry, which takes weeks to be accepted.
- Whoever may download a Working Area's files may view its previews. Across Federation, only the subject on whose behalf the delegated Run runs may view, and the hosting Node's authority decides.
- The Preview gateway records each viewing session (who, when, which Live preview and how many bytes), not each request.
- Only the Preview gateway reaches an Execution Pod, and only a Live preview's Pod on its declared port.
- The Preview gateway runs as its own role of the server binary, apart from the API server, so that untrusted traffic and long-lived WebSockets do not share its process. A Self-hosted Deployment may run both roles in one process, as with the Egress gateway.
- A preview may be framed only by the dashboard origin that requested its ticket, which under Federation is the viewer's Deployment. The dashboard shows it in a pane beside the thread and can open it in a new tab. The desktop client, whose WebView frames nothing, opens the ticket in the system browser.
- Under Federation every File preview, whatever its format, is served from the hosting Deployment's preview domain, so the ticket is the only preview mechanism Federation adds. Run inspection across Federation lists the files and the Live preview that can be viewed.

## Amendment (2026-10-26, File previews for the first useful task)

- Only Cloud must have its preview domain on the Public Suffix List; Cloud uses `aidashusercontent.app` (#535). A Self-hosted preview domain only needs to be a registrable domain separate from the dashboard's, and a Deployment refuses to start when the two share a registrable domain. Without a Public Suffix List entry, two previews of that Deployment are the same site as each other, though never as the dashboard.
- Every Deployment issues preview cookies as host-only cookies with the `__Host-` prefix, so a sibling subdomain cannot overwrite them.
- CSV and XLSX File versions are shown in the dashboard as a plain table, not on the preview domain: parsed off the main thread, with stored cell values as text, no formula recalculation, no macros, images, charts or active links, and a notice that the view is simplified. They have the same 16 MiB limit as images, plus row and cell limits; beyond them the person downloads the file.
- File previews and the HTML static site are pinned to File versions, which stay viewable while referenced (ADR 0095).
