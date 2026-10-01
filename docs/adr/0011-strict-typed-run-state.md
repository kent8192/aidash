---
status: accepted
---

# Derive Run phase from strict typed execution state

For [Issue #62](https://github.com/kent8192/aidash/issues/62), use a phase-specific `RunState` enum as the domain's source of truth and derive `RunPhase` from it, with typed waiting variants and continuations instead of independently mutable phase and pending fields. Type Run control, execution context, known history, usage, Task status and their related commands; arbitrary external content remains JSON only inside typed envelopes. This deliberately favors enforceable state structure over source, wire and persisted-format compatibility, which the user explicitly removed as a requirement.

Retain useful physical columns, including phase TEXT and pending JSONB, but encode a strict, explicitly versioned current format and update dependent APIs, clients and scheduling/trigger logic together. Unknown machine-state fields or variants, unsupported versions and malformed required data are rejected; there is no legacy converter, extension map or opaque-history compatibility variant. A private raw row/transport boundary isolates such data before it can drive execution or block unrelated Runs.

Unsupported existing nonterminal Runs enter controlled failure handling only under valid ownership and execution controls. Preserve paused Runs, live leases, terminal outcomes, Run identity and independent effect/input/delivery journals; never infer permission to replay an external effect. Inspection and stop controls remain available without restoring unsupported state. Failure and terminal delivery use validated metadata and durable dispositions, not a fabricated valid Context. Coordinated upgrades target the current format, and rollback is limited to binaries that understand it.

The user accepted Q15-Q17 on 2026-10-01 with `全て推奨`. This supersedes [ADR 0010](0010-preserve-run-state-representation.md); the [specification](../design/2026-10-01-typed-run-state-specification.md) defines the scope, persistence boundary and acceptance evidence.
