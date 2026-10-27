# File previews and Retained versions

A File preview shows one exact File version of a Working Area as a document
rather than as bytes. A Retained version keeps such a File version after a
later publication replaces it, so a correction can still show what it corrects.
A thread message quotes a File version by citing it. Live previews, which view
a Web server that sandboxed code runs, are described separately when that
feature lands.

File previews and Retained versions were decided in
[ADR 0068](adr/0068-live-previews-are-served-from-a-separate-preview-domain-one-subdomain-each-through-the-preview-gateway.md)
and
[ADR 0095](adr/0095-referenced-file-versions-are-retained-by-hold-not-copied.md).

## File previews

A File version is one immutable File of a Working Area at one path. Opening a
File preview, like downloading it, checks current authority over the Working
Area: the Workspace read permission, the `file.read` action of the
`working_area` resource, and the authority of every message or reference the
Area was built from. A File preview always opens the exact File version
selected; it never substitutes a newer one.

### Dashboard formats

The dashboard renders these File versions itself, without a preview domain:

- PNG, JPEG, GIF and WebP images.
- SVG as an `<img>`, never as inline markup.
- Markdown through a trusted renderer with raw HTML off and images resolved
  only to File versions of the same Working Area revision.
- CSV and XLSX as a simplified table, parsed off the main thread. Stored cell
  values are shown as text; formulas are not recalculated, and macros, images,
  charts and active links are not shown. The view says it is simplified.

The dashboard limit is 16 MiB per File version. A spreadsheet also stops at
10,000 rows, 1,000 columns and 200,000 cells per sheet; beyond any limit the
dashboard offers the download instead. An unsupported format explains why no
File preview exists rather than showing its bytes as text.

### HTML and PDF

HTML, XHTML and PDF File versions are served from the preview domain through
the Preview gateway, pinned to their Working Area revision and file digest.
The dashboard never renders them. An HTML File version resolves relative
references inside the same revision, and its scripts run under a Content
Security Policy that permits only that origin. A PDF opens in the browser's
viewer. The Preview gateway reads at most 64 MiB per page.

The dashboard authorizes the view exactly as it authorizes a download, then
issues a one-time ticket. The ticket is valid for one minute and is bound to
the viewer and to the dashboard origin that requested it. The Preview gateway
exchanges it for a host-only `__Host-aidash-preview` cookie on the random
subdomain of the preview domain; the cookie lasts an hour. The gateway re-runs
the download decision on each viewing request: the cookie identifies a
session, it is not authority.

Without a preview domain, HTML and PDF File versions have no File preview. The
dashboard reports that File previews are unavailable for those formats, while
the dashboard formats above still work.

## Retained versions

A Working Area keeps only its current manifest. Each publication reclaims the
File versions it replaced within seconds. A reference to a File version keeps
it by a hold instead of copying it: the immutable object stays where it is, and
reclamation skips it until the last hold covering it goes. Such a File version
is a Retained version.

### What a reference retains

- A thread message that cites a File version retains that File version while
  the message exists.
- A running Live preview retains the Working Area revision it mounts while it
  runs.
- Under Federation, the viewer's Deployment asks the hosting Node to retain a
  version for the delegated Run's subject. That hold expires after the hosting
  Node's Retention Rules period, so a lost release cannot leak storage.

Citing an HTML File version retains every File version of that Working Area's
manifest at that revision, not just the HTML file. A page commonly refers to
`../assets` and to files that later publications change; the whole revision
must stay resolvable. Citing any other format retains only that File version.

### When a hold is released

Reclamation resumes when the last hold covering a File version is released:

- A message's hold is released when the message row itself is removed, for
  example by Tenant Deletion. A person losing sight of the message does not
  release it, because others may still see it.
- A Live preview releases its revision after its Pod has stopped.
- A Federation hold is released when the viewer's Deployment says so, or
  expires after the hosting Node's Retention Rules period.
- Cleaning up a Working Area releases its holds. A recoverable cleanup keeps
  them until its snapshot expires; an irreversible cleanup removes them at
  once. Tenant Deletion removes them with the Tenant's other rows.

Releasing a hold does not delete a File version that is still in the current
manifest. The next reclamation pass erases only the versions no hold keeps and
the manifest no longer names.

### Quota

Retained versions count toward the Tenant's storage ceiling and toward the
Working Area's byte budget. Bytes of a File version still in the current
manifest are not additional retained bytes; they start counting when a later
publication supersedes that object.

A reference that would exceed either limit is refused with an explanation
instead of silently dropping or copying the version. The message can be sent
without the citation, and a Live preview does not start. The API reports a
refused citation as `FILE_VERSION_QUOTA_EXCEEDED`, naming the File version and
its path.

### "No longer retained"

A reference never points at another File version. When the hold is gone, the
citation reports `available: false` with reason `released`, and opening or
downloading that version fails as unavailable. This happens after a Working
Area cleanup or Tenant Deletion, or after an explicit release.

A viewer who may read the message but may not download the Working Area's
files sees the citation's path and ids only, with `available: false` and reason
`forbidden`; the digest, size, media type and revision are not returned. The
same current authority protects opening, downloading and subsequent viewing of
a Retained version as protects a current File version.

## Message citations

A thread message may cite up to eight File versions. Each citation names an
`area_id` and a `file_id`; the same File version cannot be cited twice in one
message.

Creating the message, in one transaction:

1. checks that the author may currently download that Working Area's files;
2. retains the File version for the message's holder;
3. records the citation with the path, digest, size, media type and Working
   Area revision seen at that moment.

A quota refusal fails the whole request with `FILE_VERSION_QUOTA_EXCEEDED`; no
message and no hold are left behind, so the message can be sent again without
the citation. Reusing a message idempotency key with a different set of
citations is a conflict.

The message creation response and the message history return each citation
with its recorded metadata and:

- `available`: whether a hold still keeps the File version;
- `reason`: `released` when the hold is gone, `forbidden` when the viewer may
  not download the Working Area.

Downloads of a cited version go through the ordinary Working Area file
download with the cited `file_id`. The Run-message summary given to the agent
names each citation as `path@digest-prefix (File version <file_id>)`, so a
correction request tells the agent exactly which File version it refers to.

## Deployment Administrator configuration

### Settings

`[preview]` and `[preview_limits]` are optional settings sections. A profile
that omits either keeps the defaults: no preview domain, and the 15-minute idle
and 2-hour absolute Live preview ceilings. The server supplies these defaults
even when the sections are absent from TOML. Every value below is optional.

```toml
[preview]
# The registrable domain File previews and Live previews are served from.
# Leave unset to offer no HTML/PDF File previews and no Live previews.
domain = "aidashusercontent.app"
# The Preview gateway listener. Defaults to 127.0.0.1:8960.
listen = "127.0.0.1:8960"
# Whether this process is also the Preview gateway. Defaults to true.
in_process = true
# The public port a ticket URL names. Defaults to 443 when absent.
public_port = 443
# The only Pod port the Preview NetworkPolicy admits. Defaults to 3000.
pod_port = 3000
```

The environment overrides are `AIDASH_PREVIEW_DOMAIN`,
`AIDASH_PREVIEW_LISTEN`, `AIDASH_PREVIEW_PUBLIC_PORT`,
`AIDASH_PREVIEW_POD_PORT` and `AIDASH_PREVIEW_IN_PROCESS`.

The Deployment's Live preview ceilings are separate, so a Tenant may lower
them without restarting the gateway:

```toml
[preview_limits]
# The longest a Live preview may sit without a gateway request.
idle_seconds = 900
# The longest a Live preview may run from its start.
absolute_seconds = 7200
```

`domain` is a bare lowercase DNS name: no scheme, port, path or wildcard. It
must be a registrable domain, and it must not share the dashboard's
registrable domain. The Deployment refuses to start when the configured
preview domain shares that domain with `deployment.endpoint`; File previews
must never be same-site with the dashboard. A local cluster sets
`domain = "localhost"`, whose File previews are served from
`https://<label>.localhost:8960`; that exception requires a local dashboard
origin (`localhost`, `127.0.0.1`, `[::1]`, `*.localhost` or a loopback IP).

Only the Cloud preview domain (`aidashusercontent.app`) must be listed on the
Public Suffix List; its DNS, certificate and list entry are tracked in
[#535](https://github.com/kent8192/aidash/issues/535). A Self-hosted preview
domain must be registrable and separate from the dashboard's, but it is not
added to the list.

Every File version or Live preview is served from its own subdomain of the
preview domain and receives a `__Host-aidash-preview` cookie. The random
subdomain carries at least 128 bits. The `__Host-` prefix requires `Secure`,
`Path=/` and no `Domain`, so the cookie is host-only; `SameSite=None` lets the
dashboard frame the File preview.

Without a preview domain, the dashboard reports File previews unavailable for
HTML and PDF, the Live preview tools are not seeded or offered to a model, and
the dashboard formats listed above still render. A Retained version can still
be downloaded.

### Helm

The chart keeps the preview domain off by default; without it there are no
Live previews and no HTML/PDF File previews.

```yaml
preview:
  enabled: true
  domain: "aidashusercontent.app"
  inProcess: true
  port: 8960
  publicPort: 443
  podPort: 3000
  limits:
    idle_seconds: 900
    absolute_seconds: 7200
  ingress:
    enabled: true
    className: nginx
    tlsSecret: aidash-preview-tls
```

- `preview.enabled` renders the Preview gateway Service, Deployment and policy.
  `preview.domain` is required when enabled.
- `preview.port` is the Preview gateway's Service port, `preview.publicPort` is
  the port a ticket URL names, and `preview.podPort` is the port a Live preview
  Pod declares. The sandbox policy below is the only ingress that admits it, so
  `live_preview_start` refuses any other port. `preview.limits` carries the
  Deployment ceilings the capability profile gives the Runner; the same values
  belong in `[preview_limits]` for the API process.
- `preview.inProcess` keeps the gateway in the API server process, which
  exposes `<release>-preview` and uses the API Server's Pod as its selector.
  Setting it to false renders a separate Deployment `<release>-preview` that
  runs `aidash preview-gateway`, turns the API process's own listener off, and
  keeps the same Service.
- `preview.ingress.enabled` renders a wildcard Ingress for `*.<domain>` and
  names its `tlsSecret`. Whoever runs the Self-hosted Deployment provides the
  wildcard DNS and certificate; Cloud's are part of #535.
- `preview.resources` sets the gateway Deployment's requests and limits.

The chart renders `<release>-preview-sandbox` in `execution.sandboxNamespace`.
It admits only this release's Preview gateway Pods, and only on
`preview.podPort`, so an Execution Pod's Web server is reachable by nothing
else. The Pod has no other ingress.

The settings example in `server/settings/base.example.toml` and the local
example in `server/settings/local.example.toml` show the same values for a file
profile instead of Helm.

## Live previews

A Live preview is a Run's long-lived Web server, mounted on one Working Area
revision. Only a Sandboxed tool call in a Run starts one, so the Run's
authority governs the code it runs; a person may stop one but never start one
outside a Run.

### Starting and stopping

- `live_preview_start { command, port }` starts the command's Web server in an
  Execution Pod and waits up to 120 seconds for the declared port to accept
  connections. It returns the preview's status and a bounded tail of the
  startup log, never an address for viewing.
- The tool is Sandboxed: the Tenant must approve the `live_preview` group
  through Catalog Approval before a Run may call it. A Deployment without a
  Runner offers no such group. Without a preview domain, neither Live preview
  tool is offered to models; with a domain, Catalog Approval still controls them.
- `port` must be the Deployment's `preview.pod_port` (default 3000), the only
  port the sandbox NetworkPolicy admits; any other port is refused with
  `PREVIEW_PORT_NOT_ALLOWED`.
- A Run has at most one Live preview. A start replaces the running one, and a
  later model turn restarts it.
- The Pod mounts the revision current at start and never publishes what its
  server writes. It does not take the Area's writer slot, so a later turn can
  run while the preview keeps serving.
- `live_preview_stop {}` and `POST /api/runs/{id}/live-preview/stop` stop it.
  The Run's requester and whoever may cancel the Run may stop it; viewing
  alone does not allow it. `GET /api/runs/{id}/live-preview` reports
  `may_stop` under the viewer's authority so the dashboard shows Stop only to
  them.

### Limits and stops

- The Deployment sets ceilings through `[preview_limits]` (`idle_seconds`
  default 900, `absolute_seconds` default 7200). A Tenant may only lower them,
  with `GET`/`PUT /api/preview-limits/{tenant}` under the same Tenant
  administration action as Workbench Test Limits. The effective values are the
  lower of the two at start.
- Every gateway request refreshes the idle timer; an active WebSocket refreshes
  it at most once a minute. The absolute limit counts from the Runner's start.
- It stops at the idle or absolute limit, when the Run is cancelled, when the
  Run's authority is revoked, when the Tenant is deleted, and when its
  Working Area is cleaned up. A later Working Area revision marks it `stale`;
  it keeps serving the revision it pinned, and its status reports both the
  stale state and that revision.

### Retention

A running Live preview retains every File version of the revision it mounts,
not only an HTML page and its siblings. Starting refuses with the quota reason
when the extra bytes would exceed the Working Area's byte budget or the
Tenant's storage ceiling. Retained bytes count only once a later publication
supersedes the object, and the hold is released only after the Runner proves
the Pod is gone; ordinary reclamation then erases what no other hold covers.

### External services

The Pod's Web server reaches the External services bound to its Run through the
Egress gateway with a session-lifetime token that ends with the Live preview.
That token resolves a bound service's Tenant Secret exactly as an operation
token does, and keeps working after the Run completes.

The viewer's page reaches the same services only through
`<preview-origin>/__aidash/egress/<service>/…`, which the Preview gateway
forwards to the Egress gateway with a separate page credential the Pod never
holds. The page path is always admitted without a Tenant Secret, because any
viewer may open the page. Both paths still require the service to be bound to
the Run and the method to be declared, and both are recorded in the Run's
egress requests; the record names the path (`operation`, `pod` or `page`).
Requests are still limited by the service's declared limits.

### Viewing

A ticket from `POST /api/previews/tickets` (File versions) or
`POST /api/previews/live-tickets` (a Live preview) opens the preview domain
after the same authority check as downloading the Working Area's files. The
Preview gateway proxies HTTP and WebSocket traffic to the Pod's declared port;
only the gateway reaches that port. Every response carries the confinement
policy, and the gateway refreshes the idle timer.

### What needs a real cluster

Starting and stopping the `serve` Pod, its readiness and log collection, the
NetworkPolicy ingress, the node guard's termination proof, and end-to-end
gateway forwarding to a real Pod and Egress gateway need a Kubernetes/gVisor
Deployment. Gateway protocol tests, domain rules, lifecycle, limits,
retention, credentials and the dashboard API run without one.
