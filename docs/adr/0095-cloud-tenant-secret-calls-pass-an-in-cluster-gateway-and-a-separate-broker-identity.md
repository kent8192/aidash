# On Cloud, Tenant Secret calls pass an in-cluster gateway and a broker identity separate from Key Material

Decided on 2026-10-26 for #310. On Cloud and Dedicated the application host can only write to the Secret Store (ADR 0002), so a Tenant tool call or an Egress gateway request that carries a Tenant Secret must have the Credential Broker attach the value (ADR 0051, ADR 0054). The Egress gateway stays in the cluster, in the server, without read access to any secret: it keeps admission, limits, recording and package-index rewriting, and forwards to the broker only the requests for an External service with a Tenant Secret. The server sends Tenant HTTP and MCP tool calls to the broker the same way. Tenant Secrets are read by a Cloud Run service and service account of their own, never by the identity that reads Key Material. Each call carries a Tenant Secret Capability Token, a kind distinct from provider tokens, that names the Tenant, the Tenant Secret and its version, the destination as an origin and path prefix, and the methods allowed. The broker forwards only paths under that destination.

## Considered Options

- **Move the whole Egress gateway to Cloud Run.** Execution Pods would need egress to Google's private API addresses, which every Google API shares, so sandboxed code could write to any Cloud Storage bucket. Rejected.
- **One broker identity for Key Material and Tenant Secrets.** Fewer resources to bootstrap, but a fault in a forwarder that sends to Tenant-chosen hosts and streams would reach provider keys. Rejected.
- **Extend the provider token's claims.** Confusing the two kinds would send a value to the wrong place, so they are kept apart by type and audience. Rejected.
- **Sign server-issued tokens with the worker's key.** Simpler IAM, but the broker could not tell issuers apart or stop one alone. Rejected: the server has its own KMS key, and the broker accepts both.

## Consequences

- Operation, Live preview and stdio-session tokens stay inside the gateway and are revoked through the database. Toward the broker, the gateway mints a 60-second token per request, cached for about 50 seconds per operation and External service. Streams that start with a valid token may run to the call's deadline, as in ADR 0005.
- The broker writes a `secret_call` log, and Aidash records the Tenant Secret, its version and the destination host on tool invocations and Egress requests, never values, header values or bodies.
- Errors that a person can repair (`secret_unavailable`, `secret_destination_refused`) are distinct from operator faults (`broker_unavailable`, `broker_rejected`). None falls back to a direct call or to reading the secret.
- Self-hosted is unchanged: the server reads the value and calls directly, with the same destination checks as the broker.
