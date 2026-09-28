---
status: accepted
---

# Preserve recipient and handler identity through overlap, edits and replay

For [Issue #70](https://github.com/kent8192/aidash/issues/70), identify recipient work by the original event, logical Agent and stable event-handler identity, retaining all matching subscription revisions as evidence rather than treating overlapping matches as additional work. Condition edits, delivery retries and historical-routing jobs preserve this identity and the existing Run/input/effect linkage; doing already completed work again requires an explicitly authorized new Task rather than resetting delivery state. This favors reproducible recovery over treating subscription revisions or replay requests as fresh idempotency namespaces, which could silently repeat external effects.
