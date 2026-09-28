---
status: accepted
---

# Separate subscription recipients from Agent definitions and workers

For [Issue #70](https://github.com/kent8192/aidash/issues/70), give each logical Agent participant an independent identity, role, subscription set and processing history even when participants share an exact Agent definition. Worker scaling changes execution capacity without creating additional recipients; deduplication must therefore preserve separate work for separate logical recipients rather than collapsing it by definition or process. Joining a Workspace presents and enables role-appropriate subscriptions, while read permission alone does not enroll an Agent, so ordinary collaboration can start through participation without silently activating every authorized reader.

Each logical participant has a distinct authority binding and role scope, bounded by the current authority of its inviting/delegating subject and the allowed capabilities of the pinned definition. Sharing a definition therefore does not share a participant's authority, and selecting a role cannot itself grant additional permissions.

Removal retires the participant; re-invitation creates a new identity. Existing deliveries and Runs retain their pinned definition while still authorized, and upgrades affect subsequent subscription revisions without rewriting prior work. Revocation blocks subsequent content access and execution; restoring authority requires explicit reconsideration of suppressed deliveries rather than silently reviving them.

[ADR-0007](0007-event-time-subscription-revisions.md) records subscription revision semantics. Acceptance of these decisions does not indicate an implemented migration or runtime path; see the [design interview](../design/2026-09-28-durable-agent-event-routing.md).
