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
one fact for each bound tool of an Agent version, and
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
methods, optional authentication and an optional daily share of its own:

```json
{
  "base_url": "https://api.example.com/v1/",
  "methods": ["GET", "POST"],
  "secret": "image-key",
  "authentication": { "type": "bearer" },
  "limits": {
    "sent_bytes": 104857600,
    "received_bytes": 104857600
  }
}
```

For another authentication header, use
`"authentication": {"type":"header", "name":"X-API-Key"}`. Connection,
routing and hop-by-hop headers cannot carry a Secret. Credentials, query
strings and fragments cannot appear in the base URL. Private destinations
need their exact origin in the Deployment's
`keyless_endpoints.permitted_private_origins`, as other custom endpoints do.

The Tenant gives the exact version a Catalog Approval and makes the service
available to a Scratch Space through the Space's `external_services` setting.
An Agent does not bind it (#738). This authorizes requests without asking for
each one: sandboxed code may send anything it can read, including Working Area
data.

Code in an Execution Pod of that Space calls the service as plain HTTP through
the proxy the Pod already has:

```python
import urllib.request
with urllib.request.urlopen("http://services.aidash.internal/images/status") as response:
    print(response.status)
```

The address is `http://services.aidash.internal/<service-id>/<relative-path>`.
It needs no token, header or environment variable. The proxy carries the Run's
credential, so the gateway knows the Run, its Scratch Space and its Tenant. It
attaches the current Tenant Secret on its own upstream HTTPS request. The hop
from the Pod to the gateway is plain HTTP inside the cluster and carries the
request and response, never the secret; a NetworkPolicy limits it to the
gateway. There is no TLS interception and no per-service host name.

On each request the gateway uses the newest version that has a Catalog Approval
covering the Scratch Space's Node. A Catalog Approval limited to certain
subjects or Agents does not count, because code in a shared Pod can claim
another Run. It also refuses the request when the Run is revoked, the Scratch
Space is not active, the Tenant or Node is deleted, or the service is no longer
on the Scratch Space. While a request is open, the gateway rechecks every four
seconds that the service is still on the Scratch Space and still approved, and
cuts the stream if not. An Egress request record names the Run and has
`via = pod`; Runner `shell` and `verify` operations and preparation commands
get no External services.

A refusal is a `403` with a JSON body:

```json
{ "error": "external_service_not_available", "available_services": ["images"] }
```

`error` is one of `external_service_not_available` (the service is not on the
Scratch Space, or does not exist), `external_service_not_approved` (no Catalog
Approval covers it on the Node), `external_service_approval_limited` (the only
approvals are limited to certain subjects or Agents) or
`external_service_method_forbidden` (the service does not allow the method).
`available_services` lists the services usable now. A Run's starting
instructions list the Scratch Space's services with their address, methods and
description.

The limits are the Scratch Space's egress allowance plus the service's own daily
share (`limits`, which a Deployment's configured ceilings cap). There is no
request count. HTTP bodies stream in both directions without content-type
restrictions or a per-request body cap. Redirects return through the gateway
and cannot change the admitted origin or escape the base path. Gateway failures
produce `EGRESS_UNAVAILABLE`; the Pod has no direct fallback route.

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
   and make it available to the Scratch Space:

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
   git -c url."http://services.aidash.internal/github/".insteadOf=https://github.com/ \
     clone https://github.com/<owner>/<repo>.git
   git -c url."http://services.aidash.internal/github/".insteadOf=https://github.com/ fetch
   ```

   The address carries no credential, so it may also be kept in the
   repository's Git configuration. The service must stay on the Scratch Space
   and approved, or the request is refused.

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

Reserve the Service IP in the cluster's Service CIDR. The Execution Pod proxy
settings point at it (`execution.pods.proxy`), and the Pod's NetworkPolicy
grants only that TCP destination. DNS, arbitrary TCP/UDP and inbound networking
stay closed. The gateway's port must remain internal to the cluster. `limits`
are the ceilings an External service's own `limits` may not exceed.

For Helm, set `execution.egress.enabled=true` and
`execution.egress.clusterIP=<reserved Service IP>`. The chart renders the
internal Service, the server ingress policy and the server and worker settings.
The gateway needs no additional secret in Execution Pods. Without egress
settings, sandbox networking remains deny-all.

## Package indexes

Agents install packages from the shell of their Execution Pod. npm, pip and uv
reach their public registries through the Pod's HTTPS proxy, limited by the
Scratch Space's allowed hosts and named by the Run's proxy credential (ADR 0136).
There are no built-in package index External services and no index rewriting
(#745, #729).

## Scratch Space proxy integration

The server connects the scope-authenticated forward proxy to Scratch Space
credentials and durable scope policy. The Pod needs
`HTTPS_PROXY` and `HTTP_PROXY` set to the internal Egress gateway. The gateway
accepts HTTP Basic proxy authentication with the scope credential as the password
(for example, `http://scope:<scope-credential>@<gateway-host>:<gateway-port>`;
URL-encode the credential). Set `NO_PROXY` only for explicitly permitted local
endpoints, for example `localhost,127.0.0.1,::1`. Do not exempt the gateway
host, metadata addresses or the cluster network: `services.aidash.internal` must
go through the proxy. The Pod's network policy must enforce the
same boundary regardless of these environment variables. The scope credential
is not a Tenant Secret, and Tenant Secret values never enter the Pod.

A new Scratch Space may reach, by default, the official package sources of
JavaScript (npm, yarn, JSR), Python (PyPI, uv's Python builds, conda-forge),
Rust, Go, Java (Maven Central, Gradle), Ruby, PHP, .NET, Dart, Elixir and R,
GitHub, Debian, Ubuntu (including arm64 ports), Alpine and the public container
registries; `DEFAULT_ALLOWED_HOSTS` in `aidash-domain` is the list. Execution
Pods set `NODE_USE_ENV_PROXY=1` besides the proxy variables, because Node's
built-in `fetch`, which corepack uses, ignores them otherwise.

For CONNECT, the gateway tunnels TLS without inspecting it. For plain HTTP it
forwards absolute-form requests. It checks the scope's host allowlist and rejects
non-public resolved addresses even when all hosts are enabled. All-host access
also requires the operator's Offering setting. The default traffic allowance is
100 GiB sent and 100 GiB received per scope per UTC day, with no request-count
limit. External service traffic is accounted in batches after forwarding; each
active stream can exceed a limit by up to one batch plus one chunk. Once a
scope or service budget is exhausted, new admissions are refused until the next
UTC day. Hosts longer than 253 bytes are rejected. Nothing stops a scope for
the number of its connections (#845, amending ADR 0111): under a host list every
other host is refused one connection at a time, so stopping would add nothing.
When a scope that may reach any host reaches more than 100 distinct hosts in a
minute, the pattern of a scan, the gateway logs a warning, counts it in
`aidash_egress_many_hosts_total` and records a `many_hosts` operator event, once
a minute at most. No consumer delivers these events yet.

During a file descriptor or memory shortage, a new Host connection or External
service request waits if its Tenant already holds more than the replica's
average number of open connections per active Tenant. The gateway rechecks the
credential while it waits. When a Scratch Space has a waiter, an idle tunnel of
that Space makes way after two seconds without activity. A tunnel awaiting a
debit is busy. Credentials resolved for new connections can be reused for five
seconds, and open tunnels recheck them independently.

The gateway tallies attempts that end before admission in
`egress_scope_refusals`. Each replica writes its tallies once a minute and on
shutdown. Each minute keeps up to 50 distinct hosts per scope. Additional hosts
share one tally. Admitted Host connections have rows in
`egress_scope_connections`. The Run's Egress waits show the host and recorded
`wait_ms` for connections that waited. Requests to External services include
their wait in admission time. See [Egress operations](operations/egress.md) for
the thresholds and metrics. DNS resolution and a stalled client write each have a 30-second
deadline. Idle tunnels close after five minutes and all tunnels close after 24
hours. The gateway reauthenticates the credential and reloads scope and host
policy every five seconds, including while idle. Revocation or a narrowed host
allowlist closes an existing tunnel. Three consecutive transient errors or timed-out
checks close it as `egress_authority_unavailable`. A slow store is not a
refusal: a traffic debit the store does not answer stops the tunnel's bytes and
is tried again, and it closes the tunnel as `egress_budget_wait_timeout` only
after 30 seconds without an answer (#909). A refund runs after the bytes have
moved, so one that fails or times out is counted in
`aidash_egress_settle_failures_total` and logged, and the connection keeps the
outcome its transfer had. Neither counts as an upstream failure. The times of
these checks, debits and refunds are in
`aidash_egress_host_connection_check_seconds`, `aidash_egress_debit_seconds`
and `aidash_egress_refund_seconds`; debits the store did not answer are counted
in `aidash_egress_debit_retries_total`. Where the store's time goes is in
`aidash_egress_store_begin_seconds` (waiting for a pooled connection and
opening the transaction; read it with the pool gauges below),
`aidash_egress_budget_lock_seconds{operation}` (waiting on the Scratch Space's
state lock, which its debits and refunds take in turn) and
`aidash_db_pool_connections{pool,state}` with `aidash_db_pool_max_connections`,
sampled every five seconds. A wait the gateway gives up on is recorded at the
time it gave up. A scope credential is a MAC over the Tenant and
Scratch Space under the server's key, verified in constant time, and is excluded
from records and logs. It has one value per Scratch Space and stays valid until
the space is deleted; deleting its Tenant or its Node closes it at once, before
any purge, and a restore reopens it. An open tunnel closes at the next recheck. A
Pod carries it in the user part of its proxy URL.

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
`unsettled`, with incomplete byte counts. A tunnel's debit against its Scratch
Space egress allowance waits at most 30 seconds and its refund five. An
interrupted connection, a refund that fails, or a debit tried again after its
answer was lost may leave bytes counted against the day's allowance that never
moved.

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
