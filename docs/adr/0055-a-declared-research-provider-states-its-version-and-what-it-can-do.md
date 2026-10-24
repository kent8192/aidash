# A declared Research provider states its version and what it can do

Decided on 2026-10-17 for #311. Whoever runs the Deployment writes each declared Research provider (ADR 0048) as `research.<name>@<n>` together with what defines that version: the protocol (`aidash.research/v1`), transport, endpoint, operations (with the MCP tool name for each), the optional search filters it honours, its display name and description, and the data-handling Research account terms. Aidash materializes the version as `aidash.research.<name>.<operation>` at `<n>.0.0` with a digest of those fields in the Entry. If a later startup finds the same `@n` with a different digest, the Node refuses to start. The price, monthly limit, account identifier, agreement and verification dates, and the Deployment secret stay outside the Entry, so changing them keeps the version and its Catalog Approvals.

A provider answers only in Aidash's result shapes or with an error code from a closed list that maps onto the host's research failures. Anything else is `provider_response_invalid`, and the attempt stays charged. A query that uses a search filter the declaration does not list is refused with `provider_request_rejected` before any approval is requested.

## Considered Options

- **Aidash bumps the version itself** when it sees changed fields, keeping a Deployment-wide table of digests. The version a Tenant approves would then change without anyone writing it down, and replicas of one Node could race to assign numbers. Rejected.
- **Re-seed silently.** An endpoint could then change under an existing Catalog Approval. Rejected.
- **Free-form provider errors and per-call filter refusal.** Text a vendor writes would reach the Run's model, and an unsupported filter would only be refused after a person approved the request and it was charged. Rejected.

## Consequences

- Agent Bindings keep pinning exact versions: after a version change a Tenant approves `@n+1` and binds it in a new Agent version; until then the old version reads `provider_unavailable`.
- Removing a declaration leaves its Entries in place but unavailable and their approvals inert; declaring the same version with the same digest again restores them.
- Replicas of one Node whose declarations disagree cannot both start.
- Brave, the page reader and the fixture are configured in the same `research_providers` settings block, keep their native implementations, and keep their reserved names. The fixture is refused in the production profile.
- The request, result and error shapes of `aidash.research/v1` are generated as JSON Schema from the domain types and committed under `docs/`; the HTTP and MCP stubs used by the acceptance tests serve as reference adapters.
