# Sandboxed code reaches the outside only through an egress gateway to declared services

Decided on 2026-10-16 for #296. Execution Pods keep deny-all networking for ingress and egress, and a Tenant Secret never enters one (ADR 0051). Code that needs an external service reaches it only through an egress gateway: NetworkPolicy lets the Pod reach the gateway alone, and the code addresses the gateway explicitly with the name of a declared external service. The gateway forwards the request to that service's `https` base URL, attaches the service's Tenant Secret, enforces limits and records every request. DNS, raw TCP or UDP and inbound traffic stay closed, and no TLS is intercepted, because the code names the gateway rather than the real destination. On Cloud and Dedicated the gateway runs with the Credential Broker's isolation on Cloud Run; on a self-hosted node it is part of the server.

## Considered Options

- **Only the model mediates**, by fetching into the Working Area with `outbound_get` and sending results out with other tools, as `python_install` already does for wheels. Each exchange costs a model step that resends the whole context, so cost grows with the number of calls times the context size. Rejected as the mechanism; it stays possible for one-off fetches.
- **Code calls Aidash tools from inside the sandbox.** Governance would stay on one path, but nested calls during a running operation make approval waits, recovery and budgets hard. Deferred until the gateway proves insufficient.
- **A transparent proxy that intercepts TLS**, so that any library and URL works under a domain allowlist. The sandbox would have to trust Aidash's CA, certificate pinning would break, and only the destination would be controlled. Rejected.
- **Execution Pods on Cloud Run.** The privileged node guard cannot run there, network control is per service rather than per Pod, and Self-hosted would need a second sandbox. Rejected; Cloud Run isolates the secret-holding gateway, not the code.

## Consequences

- An External service is a Tenant-owned Registry definition that an Agent binds and the Tenant approves through Catalog Approval. Approving it admits anything the code sends to it, as a Run grant for Web search covers queries the approver has not seen, and the approval says so. Individual requests are never approved.
- A sandbox operation authenticates to the gateway with a short-lived token for that operation, limited to the External services bound to the Run. The gateway is reachable only inside the cluster.
- Bodies stream through the gateway without a per-request size limit, so images and uploads pass unchanged. Limits apply per operation (by default 1 GiB sent, 1 GiB received, 1,000 requests, and a 30-second idle timeout). Whoever runs the Deployment sets the maximums, and an External service may only lower them, as with the Retention Rules.
- Each request is recorded without bodies, header values or query contents (only a digest of the query string), so the record shows where data went and how much, not what it was.

## Amendment (2026-10-19, Live previews)

- The package indexes are Built-in External services, and for them alone the gateway rewrites file addresses in index responses (ADR 0071).
- A Live preview's page reaches the External services bound to its Run through the Preview gateway and this gateway, without Tenant Secrets (ADR 0069). A Live preview's Web server holds a gateway token that lasts as long as the Live preview rather than one operation (ADR 0070).

## Amendment (2026-10-26, #310)

- On Cloud and Dedicated the Egress gateway stays in the cluster without read access to any secret. Only the stage that attaches a Tenant Secret runs with the Credential Broker's isolation on Cloud Run, and the gateway forwards to it the requests for External services that carry one (ADR 0095). Execution Pods still reach nothing but the gateway.
