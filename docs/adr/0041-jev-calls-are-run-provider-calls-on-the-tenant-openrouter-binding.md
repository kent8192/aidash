# Jev calls are Run provider calls on the Tenant's current OpenRouter binding

Decided on 2026-10-13 (#158, Phase 3). Jev runs through OpenRouter's System One API (ADR 0006), so every request to Jev needs a Capability Token on Cloud and a Provider Credential everywhere. Each request is a Decision Attempt of the Run whose context it reduces. Its Token Subject is that Run, not a Tenant maintenance purpose: the Decision Attempt's identifier is the call, under a new `decision` operation beside `chat`, `embeddings` and `discovery`. A Compaction Attempt cannot serve as the call: Jev pruning records none, and one pruning sends several requests in parallel. The call uses the Provider Credential that the Tenant's OpenRouter binding names at the time of the call. This holds whatever grant serves the Run's inference: an OpenRouter Provider Credential, a Plan Connection or a Keyless endpoint. Without a binding, or with a revoked credential, a Run that needs compaction stops with a typed reason and never calls Jev.

## Considered Options

- **A Tenant maintenance purpose for compaction**, as the #158 Issue text first said. Rejected: every compaction serves one Run step, and a Run subject keeps authority checks, Revocation (ADR 0013) and audit attributable to that Run.
- **Pinning a credential for Jev at Run admission**, as inference does. Rejected: Runs on Plan Connections or Keyless endpoints would need an OpenRouter binding at admission even if they never compact. Reconnecting adds a version to the same Provider Credential, so only rebinding the Tenant to a different credential mid-Run moves Jev calls to another account.
- **No compaction for Runs whose inference is not on OpenRouter.** Rejected: compaction is a separate service, not a fallback between grants. ADR 0007's rule that a Plan Connection never falls back to a Provider Credential applies to inference only.

## Consequences

- Removing the node's environment key also removes the implicit Jev pruning of ordinary Runs. A Run is pruned only through an approved compactor or Decider (ADR 0042), never because its Tenant connected OpenRouter.
- When such a Run compacts, it discloses its fitted history to OpenRouter and TypeSafe with zero data retention required, even if its inference runs on a Plan Connection or a Keyless endpoint.
- System One requests never carry `max_tokens`, so the broker checks them with their own body policy. It requires `provider.zdr = true` and bounds the request size, as for chat.
- The provider-reported cost of each request is kept in the Decision Evidence, not in a Usage Record (ADR 0042).
