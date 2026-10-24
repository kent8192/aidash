---
status: accepted
---

# Recalls preserve known stops without requiring online execution

A Recall identifies an Agent Plugin version that must not be used to start a new Run. An Agent version carrying that plugin version is refused at new-Run admission once the Deployment has learned the Recall. Recall alone does not cancel already-started Runs or delete installed files.

Deployments retain their last synchronized Recall state during communication loss. A known Recall remains effective even if the Marketplace Connection is disconnected or removed; disconnecting is not a way to clear the stop. A Recall issued while a Deployment is offline is applied after it synchronizes again. This deliberately preserves offline execution instead of promising instantaneous Recall delivery through a network partition.

These rules govern the supported runtime. As recorded in ADR 0091, a person who controls a modified Self-hosted runtime can remove its checks, so Recall is not a remote wipe or DRM guarantee for delivered content.

## Considered Options

- **Require a live Marketplace response for every new Run.** Rejected: installed local copies must not depend on Marketplace availability to keep running.
- **Clear learned Recalls when a Deployment disconnects.** Rejected: it turns disconnection into an ordinary bypass of an already-known safety restriction.
- **Promise that disconnected Deployments immediately apply new Recalls.** Rejected: communication loss prevents delivery of new information.

## Consequences

- Marketplace distributes authoritative version-specific Recall information; Deployments retain and apply it at local admission.
- A disconnected Deployment may start a Run with a version recalled elsewhere if it has not yet received that Recall. That delivery limitation is explicit.
- This records an accepted target contract; Recall synchronization is not implemented here.
