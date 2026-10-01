---
status: superseded by ADR-0011
---

# Preserve the persisted Run representation while typing continuation state

The user withdrew the backward-compatibility requirement on 2026-10-01 with `後方互換を無視する`. This record describes the earlier decision and is no longer authoritative for representation, legacy defaults or unknown-field preservation. [ADR 0011](0011-strict-typed-run-state.md) replaces it. The whole-state typing scope and controlled invalid-row handling remain accepted.

For [Issue #62](https://github.com/kent8192/aidash/issues/62), represent continuation state with a typed struct and typed nested records while retaining the existing flat JSON keys and database columns. Ordinary updates preserve unknown fields without using them to authorize or select execution; existing explicit continuation resets retain their existing cleanup semantics. This favors recovery of valid existing Runs and preservation of extension data over introducing a waiting-reason discriminator or silently discarding fields during deserialization.

Arbitrary external payloads remain JSON values within typed envelopes. Missing fields retain their existing interpretation, while malformed known state fails through controlled Run handling rather than coercion or a worker panic. The implementation must isolate invalid persisted rows before typed decoding or database scheduling expressions can prevent unrelated Runs from progressing, while preserving lease fencing, terminal delivery and uncertain-effect history.

The boundary includes Run phase, control, execution context and pending state, Task status, and related control/transition arguments. Preserve missing fields, explicit nulls and present values distinctly where the legacy representation admits them; typed nested records also retain their extension fields. Rust source compatibility is allowed to change, while valid stored and wire representations stay compatible. A private raw persistence/transport representation exists only at the boundary and must never be used to drive ordinary execution before validation.

The user accepted Q2-Q9 on 2026-10-01. Remaining lifecycle and verification decisions are tracked in the [design interview](../design/2026-10-01-typed-run-state.md).
