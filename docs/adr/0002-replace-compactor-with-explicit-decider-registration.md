---
status: accepted
---

# Replace compactor with explicit decider registration

Issue #108 adopts explicit new Decider registrations instead of preserving legacy compactor definitions, environment selection, or generation-policy references through projection or automatic migration. This accepts a breaking cutover and requires owners to declare and approve the new dependencies, avoiding a second configuration path whose authority and version semantics could diverge. The Issue's original compatibility acceptance criteria must be revised to match this decision.
