---
status: accepted
---

# Route live events using the subscription revision effective at commit

For [Issue #70](https://github.com/kent8192/aidash/issues/70), determine a live event's candidate recipients and handler configuration from the subscription revisions effective when that event committed, with activation and edits recorded at explicit sequence boundaries. Broker delays must not let later subscription broadening silently acquire earlier events or make the original recipient set depend on recovery timing; current authorization and enablement still gate content exposure and execution, so historical eligibility never preserves revoked authority. This requires retaining sufficient subscription history and choosing a durable activation boundary instead of merely querying the latest subscription configuration during delivery.
