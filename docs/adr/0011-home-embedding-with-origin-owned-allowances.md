---
status: accepted
---

# Execute embeddings at Home without moving generation allowances

For [Issue #75](https://github.com/kent8192/aidash/issues/75), the Home node invokes and pays for the embedding service used by its Workspace index; its service credentials stay at Home. Generated Agents must explicitly approve that exact Home provider and configuration, and each generated ancestor's originating authority must reserve its applicable call and token allowance before provider dispatch. This preserves both the source owner's provider choice and the generator's resource limits: running a query at Home cannot erase execution-node generation constraints, and failed or uncertain dispatched attempts remain charged.

The accepted [specification](../design/2026-09-30-remote-semantic-memory-specification.md) defines durable reservations and recovery. A local-only reservation implementation is not evidence of cross-node enforcement.
