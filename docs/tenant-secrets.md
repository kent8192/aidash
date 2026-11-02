# Tenant Secrets and sandbox egress

Tenant Secrets are named, write-only values for a Tenant's HTTP/MCP tools,
Workbench test endpoints and External services. They share the Secret Store
with Provider Credentials and Plan Connections, under a separate
`aidash-store/tenant-secrets/<id>` resource namespace. They are available on
Self-hosted nodes with an encrypted PostgreSQL Secret Store, and on Cloud and
Dedicated where a Secret Manager Store and a Tenant Secret broker are both
configured ([#310](https://github.com/kent8192/aidash/issues/310)). Without the
broker, Cloud and Dedicated keep them unavailable.

## Manage and use a Tenant Secret

In Tenant settings, a subject with `secret.manage` stores a name, a value and
the hosts permitted to receive it. Names contain 1–64 lowercase letters,
digits, underscores or hyphens. Record bare hosts, without a scheme, port,
path or wildcard; a recorded host permits any port on that host. Only the
name, hosts, fingerprint, last four characters and revision are returned.
Values never enter browser persistence, audit events or execution inputs.
A Deployment Administrator cannot manage another Tenant's secrets.

The metadata API is `GET /api/tenants/{tenant}/secrets` and
`GET /api/tenants/{tenant}/secrets/{name}`. A metadata reader needs
`secret.use` or `secret.manage`. `PUT` on the latter path stores or replaces a
value; `DELETE` removes it. Both require `secret.manage` and an
`expected_revision` (`0` creates a new name). A PUT accepts `value`, `hosts`
or both; a new name requires both. The value is never returned. Changing
hosts or replacing a value increments the Secret's revision without changing
any definition version. On Cloud and Dedicated, changing a Secret's hosts
requires entering its value again in the same request (ADR 0096); on
Self-hosted, hosts change without the value. Tenant purge removes its values
before its rows.

Readiness is read-only and never returns a value, a Fingerprint or the last
four characters. `GET /api/registry/{id}/{version}/secret-readiness` returns
one fact for each bound tool and External service of an Agent version, and
`GET /api/marketplace/packages/{key}/secret-readiness` returns one fact for
each Secret slot the package's install plan declares. A fact names its item,
`kind` (`tool`, `external_service` or `slot`), the `tenant_secret_name` when
one is named, a `status` (`ready`, `secret_missing`,
`secret_hosts_mismatch`, `secrets_unavailable_on_deployment` or
`broker_not_configured`) and who can `repair` it (`manage_secret`,
`ask_administrator`, `deployment_administrator` or `none`). A slot is ready on the plan screen
when any active Tenant Secret records the package's destination host; after
installation the installed copy names its chosen Secret.

Naming a Secret in a definition requires `secret.use`, independently of
`secret.manage`. HTTP/MCP tool transports use `secret: "search-key"` instead
of a Deployment environment reference. A Workbench Test Profile rule uses the
same field. Registration, installation, profile admission and dispatch all
check that the destination's host is recorded by the Tenant Secret. Missing,
deleted or no longer permitted Secrets fail dispatch with
`secret_unavailable`.

A Marketplace package declares the Secret names it needs in `secrets`; the
installer assigns one of their own Tenant's Secrets to each name. Installation
checks the resulting destination against those hosts. Catalog Approval shows
the Secret-to-host disclosure and states that Aidash's provider guarantees,
including zero data retention and Usage Records, do not apply. Federation
executes the tool at its owning Node; Secret values stay there.

## Cloud and Dedicated

The application host stays write-only (ADR 0002). A Tenant Secret lives in the
BYOK project under `aidash-<environment>-tsec-<id>`, separate from Key
Material's `-cred-` namespace. Each Secret Manager version is the sealed
payload `{"v":1,"hosts":[...],"value":"..."}` (ADR 0096): the hosts travel with
the value, so the Tenant Secret broker refuses any destination outside them
and the application host cannot attach new hosts to a value it cannot read.
Creating a Secret is a `pending` row followed by the Secret Manager write, so a
reconciler finds orphans by scanning PostgreSQL (ADR 0002).

A tool call, MCP request or Egress request that carries a Tenant Secret is
forwarded to the broker instead of being sent directly. The server mints a
short-lived Tenant Secret Capability Token that names the Tenant, the Secret,
its pinned version, the destination origin and path prefix, the methods and
the attachment; the broker reads only that version, opens the sealed hosts and
attaches the value. The token is sent as `x-aidash-secret-call`, never as
`Authorization`. The server signs with its own KMS key; workers reuse their
existing Credential Broker key with the Tenant Secret broker's audience. Both
public keys and their issuers are configured on the Tenant Secret broker.

The application host never reads a version, and a failed broker call never
falls back to a direct read. A person can repair `secret_unavailable` and
`secret_destination_refused`; `broker_unavailable` and `broker_rejected` are
Deployment Administrator faults. Tenant users see the Deployment Administrator-side message and a correlation
id; only Deployment Administrators see reason codes.

Configure the broker with `providerCredentials.settings.tenant_secret_broker`:

```json
{
  "endpoint": "https://aidash-tenant-secret-broker-...run.app",
  "issuer": "aidash-server",
  "audience": "production-tsec",
  "kid": "projects/.../cryptoKeys/tenant-secret/cryptoKeyVersions/1"
}
```

`endpoint` is the bare service URI; the server appends `/v1/forward`. The
`audience` must be `<environment>-tsec`, distinct from the provider token
audience. Without this setting, Tenant Secrets stay unavailable on Cloud and
Dedicated, and definitions that name one are refused at registration.

## External services

An `external_service` Registry entry has one HTTPS base URL, explicit HTTP
methods, optional authentication and optional lower per-operation limits:

```json
{
  "base_url": "https://api.example.com/v1/",
  "methods": ["GET", "POST"],
  "secret": "image-key",
  "authentication": { "type": "bearer" },
  "limits": {
    "sent_bytes": 104857600,
    "received_bytes": 104857600,
    "requests": 100
  }
}
```

For another authentication header, use
`"authentication": {"type":"header", "name":"X-API-Key"}`. Connection,
routing and hop-by-hop headers cannot carry a Secret. Credentials, query
strings and fragments cannot appear in the base URL. Private destinations
need their exact origin in the Deployment's
`keyless_endpoints.permitted_private_origins`, as other custom endpoints do.

The Tenant gives the exact version a Catalog Approval and binds it to an
Agent with `kind: "external_service"`. This authorizes requests without asking
for each one: sandboxed code may send anything it can read, including Working
Area data. Revoked Catalog Approval, changed definitions, cancelled operations
and revoked Run authority fail closed at the gateway.

Inside an operation, `AIDASH_EGRESS_URL` holds its authenticated gateway base.
Code appends `<service-id>/<relative-path>`, for example:

```python
import os, urllib.request
with urllib.request.urlopen(os.environ["AIDASH_EGRESS_URL"] + "images/status") as response:
    print(response.status)
```

The environment contains an expiring operation token, never the Tenant Secret.
The gateway attaches the current Secret only on its upstream request. HTTP
bodies stream in both directions without content-type restrictions or a
per-request body cap. The default cumulative budget per operation is 1 GiB
sent, 1 GiB received and 1,000 requests; a service may lower those limits.
Counters persist in PostgreSQL and are serialized on the operation, so
concurrent requests and gateway restarts share the same budget. The operation
deadline bounds each stream and 30 seconds without data ends a connection.
Redirects return through the gateway and cannot change the admitted origin or
escape the base path. Gateway failures produce `EGRESS_UNAVAILABLE`; the Pod
has no direct fallback route.

The gateway debits body bytes in batches of 1 MiB per direction, and debits
the remainder when a stream ends, fails or is interrupted, so the request
record always holds the bytes actually streamed. A debit commits those bytes
even when they take a counter past its limit, then stops the stream. A budget
can therefore be exceeded by at most one batch (1 MiB plus one body chunk) per
direction for each request streaming at that moment; once a counter is
exhausted, the next request is refused before it starts. The stream's error,
in both the request record and the client response, names the reason:

| Code                    | Status | Meaning                                                           |
| ----------------------- | ------ | ----------------------------------------------------------------- |
| `egress_limit_exceeded` | 429    | A byte or request budget is exhausted.                            |
| `operation_expired`     | 403    | The operation or Live preview ended, expired or no longer exists. |
| `egress_forbidden`      | 403    | The Run's authority was revoked.                                  |
| `EGRESS_UNAVAILABLE`    | 503    | The budget store could not record the debit.                      |

A response whose headers were already sent ends with an incomplete body
instead of a status.

Run inspection lists the request time, operation, service, method, path,
SHA-256 query digest, status, byte counts and duration. Bodies, header values
and query contents are never recorded. A partially consumed response is
recorded as interrupted rather than silently becoming a successful request.

### Clone a GitHub repository

The Execution Pod image includes `git`. It reaches GitHub through an External
service; the gateway attaches the Tenant Secret, so Git needs no credential
helper and the token never enters the Pod.

1. Store a Tenant Secret, for example `github-git`, with hosts `["github.com"]`
   and the value `Basic <base64(x-access-token:TOKEN)>`. The base64 text encodes
   the literal username `x-access-token`, a colon and the token. Cloning needs
   read access to the repository's contents; pushing needs write access.
2. Register an `external_service`, for example with ID `github`, approve it
   and bind it to the Agent:

   ```json
   {
     "base_url": "https://github.com/",
     "methods": ["GET", "POST"],
     "secret": "github-git",
     "authentication": { "type": "header", "name": "Authorization" }
   }
   ```

3. Clone, fetch and push with the ordinary `https://github.com/` URL. Map it to
   the gateway with `-c` on every command:

   ```sh
   git -c url."${AIDASH_EGRESS_URL}github/".insteadOf=https://github.com/ \
     clone https://github.com/<owner>/<repo>.git
   git -c url."${AIDASH_EGRESS_URL}github/".insteadOf=https://github.com/ fetch
   ```

   `AIDASH_EGRESS_URL` carries an operation token that ends with the operation.
   Never write it into the repository's remote URL or Git configuration: a later
   operation gets a new token, and the stored one fails.

Archive downloads do not work. GitHub redirects them to the cross-origin
`codeload` host, and the gateway refuses cross-origin redirects. Git's
`info/refs` and `git-upload-pack` requests to a private repository were
verified through the gateway. A complete clone of a large repository is still
limited by [#683](https://github.com/kent8192/aidash/issues/683) (throughput)
and by the Working Area's file rules
([#682](https://github.com/kent8192/aidash/issues/682)).

## Self-hosted deployment

The gateway is an internal listener of the server. The capability profile has
an optional `egress` section:

```json
{
  "endpoint": "http://10.96.0.90:8950",
  "listen": "0.0.0.0:8950",
  "namespace": "aidash",
  "pod_selector": {
    "app.kubernetes.io/instance": "aidash",
    "app.kubernetes.io/component": "server"
  },
  "limits": {
    "sent_bytes": 1073741824,
    "received_bytes": 1073741824,
    "requests": 1000
  }
}
```

Reserve the Service IP in the cluster's Service CIDR. The Runner needs the same
endpoint and gateway pod selectors; it grants only that TCP destination in
the sandbox's NetworkPolicy. DNS, arbitrary TCP/UDP and inbound networking
stay closed. The gateway's port must remain internal to the cluster.

For Helm, set `execution.egress.enabled=true` and
`execution.egress.clusterIP=<reserved Service IP>`. The chart renders the
internal Service, server ingress policy and shared server/Runner settings.
The server and workers retain the same Runner credential used to sign
operation tokens. The gateway needs no additional secret in Execution Pods.
Without egress settings, sandbox networking remains deny-all.

## Package indexes

The Runner image includes Node.js 22 LTS, npm/npx and uv/uvx. Built-in
External services `core.npm` and `core.pypi` need their own Tenant Catalog
Approvals and Agent bindings. The operation points npm's registry and uv's
index at the gateway. npm's `replace-registry-host` stays `npmjs`, so lockfile
links on registry.npmjs.org go through the gateway while the gateway's own
rewritten links are used unchanged. Python downloads are disabled with
`UV_PYTHON_DOWNLOADS=never`.

Only these built-in indexes buffer index responses, up to 8 MiB, to rewrite
package file addresses onto the gateway. npm files remain on
`registry.npmjs.org`; PyPI files may also use `files.pythonhosted.org`.
Integrity fields and URL hash fragments remain intact, and package files
stream unchanged under the operation's cumulative budget. `python_install`
and its wheel-selection API/UI have been removed; use uv through the gateway.

File previews, Retained versions and message citations are described in
[File previews and Retained versions](file-previews-and-retained-versions.md).
Live previews follow with [#405](https://github.com/kent8192/aidash/issues/405).
