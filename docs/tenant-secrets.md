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

| Code                    | Status | Meaning                                                    |
| ----------------------- | ------ | ---------------------------------------------------------- |
| `egress_limit_exceeded` | 429    | A byte or request budget is exhausted.                     |
| `operation_expired`     | 403    | The operation or Live preview expired or no longer exists. |
| `operation_inactive`    | 403    | The Run or operation is no longer active.                  |
| `egress_forbidden`      | 403    | The Run's authority was revoked.                           |
| `EGRESS_UNAVAILABLE`    | 503    | The budget store could not record the debit.               |

A response whose headers were already sent ends with an incomplete body
instead of a status. A temporary `EGRESS_UNAVAILABLE` in the middle of a download
also stops the stream, but keeps its unbilled bytes and retries them in the
background. The last debit of a response is different: the body has already been
delivered, so the response ends normally and the gateway retries that debit, and
writes the request record, in the background.

A refusal is answered at once, and the gateway then discards what the client
still sends, up to 8 MiB or 5 seconds, so that the client can read the answer.
Discarded bytes are not charged. A client that sends more before it reads may
see a reset instead of the refusal, and the cut is counted by
`aidash_egress_linger_limit_total`. The time limit is counted when bytes were
discarded and the declared request body remains incomplete. A client that sends
nothing during linger is not counted.
The reason reaches a request record only when one exists: a request refused
before its record is created leaves only the count
in `aidash_egress_refused_total`, and a connection refused for want of a slot
only the count in `aidash_egress_connections_refused_total`.

A response with a `Content-Length` settles after its last byte is handed on. A
final debit that reaches a limit is recorded but can no longer stop that
response; the next request is refused.

The gateway serves 128 connections at once. Beyond that it answers
`503 egress_connection_limit` within 2 seconds, then discards what the client
still sends like any other refusal; when too many refusals are in progress it
drops the connection unanswered.

Run inspection lists the request time, operation, service, method, path,
SHA-256 query digest, status, byte counts and duration. Bodies, header values
and query contents are never recorded. A partially consumed response is
recorded as interrupted rather than silently becoming a successful request.
A response counts as complete once all its bytes are handed to the server's
HTTP layer, so a client that disconnects at the very end can still appear as a
success.

A record starts with status 0 and no error; its outcome is unsettled. The final
debit and record are retried for about 60 seconds when storage is unavailable,
so a record that stays at status 0 has
an unknown outcome: the process stopped first or storage stayed unavailable. A
debit that is still unavailable after those 60 seconds is given up: its bytes are
not charged, and a record that can still be written shows the error
`egress_debit_abandoned`, so that its byte counts are not mistaken for complete.
A decision about that debit, such as `egress_limit_exceeded`, is recorded instead,
and a body that did not reach its end stays `stream_interrupted`.

A settled record also shows what the request cost at the gateway, in the Run's
`egress_requests`. `admission_ms` is the admission call: the token, the
authority's recheck, the budget locks and the record's first write. `settle_ms`
runs from the end of the response until its final debit settled, including a
first attempt that failed and the retries after it, or until the debit was given
up. Bytes kept when a debit failed in the middle of a stream are charged later in
the background and are not counted. `duration_ms` starts after admission and
includes the final debit only for a response that ends its own stream and whose
debit did not fail. A record that never settled has neither time, and a request
refused before its record exists has no record.

Failures that leave no record are visible to the operator of the Deployment.
Unexpected admission failures and abandoned retries log an error, and upstream
failures log a warning. These lines name the service, method and cause, never
the token, path, query or header values. Sandboxed code sees only the code in
the table above. The metrics listener (`AIDASH_METRICS_LISTEN`) exposes:

| Counter                                   | Labels             | Counts                                               |
| ----------------------------------------- | ------------------ | ---------------------------------------------------- |
| `aidash_egress_refused_total`             | `reason`           | Requests refused before a record exists.             |
| `aidash_egress_connections_refused_total` | `outcome`          | Connections `answered` or `dropped` when full.       |
| `aidash_egress_settle_failures_total`     | `stage`, `outcome` | Failed debits and records: `retried` or `abandoned`. |
| `aidash_egress_linger_limit_total`        | `limit`            | Uploads cut off after a refusal: `bytes` or `time`.  |

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
package file addresses onto the gateway. An index exceeding this limit is refused
with HTTP 422 and the message `package_index_limit_exceeded: this package index is over 8 MiB. Install from a lockfile, or by tarball URL through the registry.` in both the error body and HTTP reason phrase. npm files remain on
`registry.npmjs.org`; PyPI files may also use `files.pythonhosted.org`.
Integrity fields and URL hash fragments remain intact, and package files
stream unchanged under the operation's cumulative budget. `python_install`
and its wheel-selection API/UI have been removed; use uv through the gateway.

File previews, Retained versions and message citations are described in
[File previews and Retained versions](file-previews-and-retained-versions.md).
Live previews follow with [#405](https://github.com/kent8192/aidash/issues/405).

## Scratch Space proxy integration

The server connects the scope-authenticated forward proxy to Scratch Space
credentials and durable scope policy. The operation-token gateway described
above also remains available. The Pod needs
`HTTPS_PROXY` and `HTTP_PROXY` set to the internal Egress gateway. The gateway
accepts HTTP Basic proxy authentication with the scope credential as the password
(for example, `http://scope:<scope-credential>@<gateway-host>:<gateway-port>`;
URL-encode the credential). Set `NO_PROXY` only for explicitly permitted local
endpoints and the exact gateway host, for example
`localhost,127.0.0.1,::1,<gateway-host>`. The gateway exemption lets callers
reach its path-level External service route directly. Do not exempt metadata
addresses or the cluster network. The Pod's network policy must enforce the
same boundary regardless of these environment variables. The scope credential
is not a Tenant Secret, and Tenant Secret values never enter the Pod.

For CONNECT, the gateway tunnels TLS without inspecting it. For plain HTTP it
forwards absolute-form requests. It checks the scope's host allowlist and rejects
non-public resolved addresses even when all hosts are enabled. All-host access
also requires the operator's Offering setting. The default traffic allowance is
100 GiB sent and 100 GiB received per scope per UTC day, with no request-count
limit. External service traffic is accounted in batches after forwarding; each
active stream can exceed a limit by up to one batch plus one chunk. Once a
scope or service budget is exhausted, new admissions are refused until the next
UTC day. The default anomaly rule suspends the scope after more than 600 new
authenticated connection attempts in one minute, including invalid targets and
hosts denied by the allowlist; an operator must lift the suspension. Hosts longer than 253 bytes are rejected. The
authority must emit an operator notification when it suspends a scope.

Each gateway instance defaults to 64 simultaneous tunnels per scope, 32 per
Tenant and 1,024 overall. The Tenant and global limits can be configured in the
capability profile; the Tenant limit must be positive and lower than the global
limit. Capacity exhaustion returns `egress_tunnel_limit`,
`egress_tenant_tunnel_limit` or `egress_global_tunnel_limit`. DNS resolution and a stalled client write each have a 30-second
deadline. Idle tunnels close after five minutes and all tunnels close after 24
hours. The gateway reauthenticates the credential and reloads scope and host
policy every five seconds, including while idle. Revocation or a narrowed host
allowlist closes an existing tunnel. Three consecutive transient errors or timed-out
checks close it as `egress_authority_unavailable`; they are not reported as a
suspension. A traffic debit that fails because the store is unavailable ends it
the same way, and a refund that fails ends it as `egress_refund_failed`; neither
counts as an upstream failure. A scope credential is a MAC over the Tenant and
Scratch Space under the server's key, verified in constant time, and is excluded
from records and logs. It has one value per Scratch Space and stays valid until
the space is deleted; deleting its Tenant or its Node closes it at once, before
any purge, and a restore reopens it. An open tunnel closes at the next recheck. A
Pod carries it in the user part of its proxy URL. Agent execution in Execution
Pods follows in wave 2 of #697.

Connection and event row retention remains a follow-up; operators must account
for their storage until an automatic retention policy is implemented.

Connection records contain scope, host, port, sent and received byte counts,
duration and outcome, rather than request paths or bodies. Invalid targets are
recorded when a bounded host and port can be parsed; malformed targets without
one are rejected without retaining the raw request. A record is created
before the connection starts, with the outcome `unsettled`, and updated when it
ends. `interrupted` means that the connection was cut other than by its client.
`egress_client_closed` means that the client reset it or stopped reading, which
may be a finished download or an abandoned one, since a tunnel cannot see inside
TLS. If the initial
record cannot be written after retries, the gateway refuses the connection with
`egress_audit_unavailable`. The final record is retried for about 60 seconds
without holding the connection's concurrency slot, or once more if the process
is stopping. A process crash or sustained audit-store failure can leave a record
`unsettled`, with incomplete byte counts. Tunnel budget reservations and
refunds each have a five-second deadline. An interrupted connection or timed-out
refund may retain unused reserved budget.

Sandboxed code can repeat failures of a Host connection at will, so each kind of
failure logs at most one line per 10 seconds, with the number since the last
line. The rest appear at `debug`. Unexpected admission failures and abandoned
records log an error, and upstream failures log a warning. These lines name the
kind, cause and connection record's ID, never the token or host: the record has
the host, and under "allow all hosts" a Pod chooses it freely.
`aidash_egress_host_connection_failures_total{kind}` counts every upstream
failure, with `kind` one of `dns`, `connect`, `handshake`, `request`, `upgrade`
or `transfer`. `transfer` counts only ends with no named code, and one failed
connection can count under both `transfer` and `request`.

The separate path-level External service route remains responsible
for attaching Tenant Secrets; scope-based admission for that route is refused
until #738. Its integration route is `/v2/services/<service>/<path>` with the scope
credential in `Authorization: Bearer <scope-credential>`. The gateway removes
that header before forwarding and attaches the External service's Tenant Secret
separately. The legacy `/v1/<operation-token>/<service>/<path>` route continues
to use operation admission until the scope authority is connected.
