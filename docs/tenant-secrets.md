# Tenant Secrets and sandbox egress

Tenant Secrets are named, write-only values for a Tenant's HTTP/MCP tools,
Workbench test endpoints and External services. They share the Secret Store
with Provider Credentials and Plan Connections, under a separate
`aidash-store/tenant-secrets/<id>` resource namespace. They are available on
Self-hosted nodes with an encrypted PostgreSQL Secret Store. Cloud and
Dedicated forwarding belongs to [#310](https://github.com/kent8192/aidash/issues/310).

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
any definition version. Tenant purge removes its values before its rows.

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

Run inspection lists the request time, operation, service, method, path,
SHA-256 query digest, status, byte counts and duration. Bodies, header values
and query contents are never recorded. A partially consumed response is
recorded as interrupted rather than silently becoming a successful request.

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
Approvals and Agent bindings. The operation sets npm's registry and
`replace-registry-host`, and uv's index, to the gateway. Python downloads are
disabled with `UV_PYTHON_DOWNLOADS=never`.

Only these built-in indexes buffer index responses, up to 8 MiB, to rewrite
package file addresses onto the gateway. npm files remain on
`registry.npmjs.org`; PyPI files may also use `files.pythonhosted.org`.
Integrity fields and URL hash fragments remain intact, and package files
stream unchanged under the operation's cumulative budget. `python_install`
and its wheel-selection API/UI have been removed; use uv through the gateway.

File previews, Retained versions and message citations are described in
[File previews and Retained versions](file-previews-and-retained-versions.md).
Live previews follow with [#405](https://github.com/kent8192/aidash/issues/405).
