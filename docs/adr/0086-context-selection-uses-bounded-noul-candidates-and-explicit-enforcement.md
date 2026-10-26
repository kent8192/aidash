# Context selection uses bounded Noul candidates and explicit enforcement

Decided on 2026-10-23 for #103 and #110. Registry-declared context sources contribute through a shared port. Source pinning supports Always, OnDemand and explicitly selected Scored behavior. Scored selection supports both Shadow and Enforce; existing default query, refresh and request-rendering behavior remains unchanged.

The selector receives a bounded preview of each authorized candidate, its exact identity, revision and provenance, and the source policy's declared query material. Sending that state requires disclosure authority independently of permission to read the source. Source text remains attributed data and cannot grant authority or become privileged instructions. Each bounded question asks a Noul probability of keeping one identified candidate. Threshold application, stable ordering, tie-breaking, deduplication and final request fitting remain deterministic code. This use does not require adding unused Choice or generic Score answer contracts.

Enforce selection that cannot obtain its approved answer because of unavailable service, missing approval, forbidden disclosure or exhausted budget pauses with a typed reason. It never silently becomes Always selection. Shadow records proposed selections or their unavailability and leaves the request unchanged. The source provider's existing unavailable policy, including ADR 0059's narrowly permitted workspace degradation, is separate from selection failure; required-Home retrieval and mandatory receipts remain mandatory.

Fetched candidates, selected items, fitted contributions and content delivered to an accepted inference are separate stages. Selection evidence proves a decision, not a complete read. Bounded contributions retain a revision-bound detail path, completeness and truncation information, provenance and read dependencies through fitting and compaction. Registry metadata contains declarations and configuration references, while private payloads remain in authorized provider stores.

## Considered options

- Send only metadata: rejected because it cannot establish whether the candidate's content is relevant.
- Send complete source stores or the entire Context Journal: rejected because a bounded authorized preview is sufficient and limits disclosure.
- Fall back to unscored inclusion after an Enforce failure: rejected because it changes the explicitly selected behavior without a decision.
