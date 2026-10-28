# Default compaction Bindings cut over with the undeployed schema

Decided on 2026-10-23 for #103 and #108. Aidash has not been deployed. The remaining Registry-centric Harness work therefore uses one current Binding schema, rather than adding compatibility projections, a normalization-version bridge or a migration of existing Agent versions and Run snapshots. Development data that does not satisfy the new schema must be recreated; immutable records are never silently rewritten.

Every ordinary Agent receives the exact built-in compaction Decider through trusted registration normalization unless it explicitly removes that default or selects another compaction Decider. Registration records the complete normalized dependency graph, and a Run pins that graph. The built-in declaration contains the Routes approved in ADR 0042 and ADR 0066; declaration and default Binding never substitute for the Tenant's Catalog Approval or its current Provider Credential Binding. Unapproved or unavailable compaction keeps the existing typed pause behavior. (2026-10-27, #540) ADR 0101 moves the approval check up front: the Agent's Approval Review includes the default Decider and a Run whose compaction Decider is unapproved does not start; the typed pause remains for a withdrawn approval and for an unavailable Decider.

Generated chains retain ADR 0042's exception: they use only the exact Decider named by their generation policy and accepted by every generated ancestor. A schema reset does not give them an implicit default or weaken their call budgets.

## Considered options

- Version normalization and preserve the old closure for existing Agent versions: rejected for this undeployed cutover, as confirmed by the repository owner.
- Recompute and overwrite existing immutable definitions or snapshots: rejected because it would change the dependencies their identities commit to.
