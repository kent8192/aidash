# Typed Run execution state

Source: [Issue #62](https://github.com/kent8192/aidash/issues/62).
Inspected base: `develop/0.1.0` at `3cb803544c37ebbeb413bc8554accb0a06d916fa`.
Status: design accepted on 2026-10-01 and implemented locally on `feat/issue-62-typed-run-state`. The [specification](2026-10-01-typed-run-state-specification.md) is the consolidated contract; [validation](2026-10-01-typed-run-state-validation.md) records the implementation and local checks.

## Current direction

The latest user instruction is `後方互換を無視する` (ignore backward compatibility). It supersedes the earlier promises to resume legacy JSON unchanged, retain unknown fields, preserve missing/null distinctions and timestamp spellings, retain wire layouts and support previous binaries. It does not authorize deleting existing records or repeating external effects.

The accepted whole-state typing scope, arbitrary payloads within typed envelopes, narrow continuation destinations and lease/revision-fenced invalid-state isolation remain in force. Existing business transitions, authorization, uncertain-effect reconciliation and terminal delivery remain the behavioral baseline.

The final decision is a phase-specific `RunState` enum, derived `RunPhase`, typed waiting variants and strict current-format Context/history. Physical columns remain useful storage projections, while JSON/API layouts may change and stored state carries an explicit format version. Old-format Runs do not resume; Q17 defines controlled failure and retention. The unconfirmed Q10-Q14 recommendations from the previous round are not accepted decisions; their compatibility assumptions are withdrawn and their remaining concerns are covered by Q15-Q17 and the specification.

## Inspected baseline evidence

These findings describe the inspected base above, before implementation.

- `src/domain.rs` already defines `RunPhase`, but `Run.phase` is a `String`. `TaskStatus` is also defined while `Task.status` remains a `String`.
- `Run.control` is a `String`; its database values are `ACTIVE`, `PAUSED` and `CANCELLED`.
- `Run.context` and `Run.pending` are JSON values. A `Context` struct exists, but runtime code repeatedly decodes and re-encodes it; its history and usage still contain untyped state.
- Continuation state is consumed by Harness, Store, Federation and transaction completion. Worker activation and lease eligibility also read JSON keys through database expressions.
- Waiting/approval cleanup still contains two `as_object_mut().unwrap()` calls.
- Lease acquisition directly decodes a row as `Run`. Strict decoding without a separate invalid-row path can repeatedly reject the same row before ordinary Run failure handling starts.
- Scheduling expressions cast `retry_at` and `wake_at` to PostgreSQL timestamps and test key presence. Shape validation alone does not isolate all scheduling failures or preserve missing/null semantics.

## Round 1: accepted decisions

| Question | Decision | Status |
| --- | --- | --- |
| Q1: Typing scope | The user requested typing the whole state surface rather than restricting this work to phase and pending. Q5 fixes the concrete boundary. | Accepted; expanded by Q5 |
| Q2: Continuation shape | Use a typed struct corresponding to the existing flat JSON, with nested typed records for machine-controlled state. Keep arbitrary tool arguments and human response contents as JSON inside those records. | Flat representation superseded by Q15; typed payload boundary retained |
| Q3: Compatibility | Preserve existing key names, values and missing-field interpretation. Preserve unknown fields on ordinary updates without executing them; existing explicit continuation resets retain their cleanup semantics. Add no discriminator or database migration. | Superseded by removal of backward compatibility |
| Q4: Malformed persisted state | Do not coerce malformed known state into defaults. Isolate it and use controlled Run failure handling, preserving cancellation precedence, lease fencing, terminal delivery and uncertain-effect evidence. Unrelated Runs continue. | Accepted |

Q1 response: `全体を型付けにする`. Q2-Q4 response: `その他は推奨`.

## Round 2: accepted decisions

| Question | Decision | Status |
| --- | --- | --- |
| Q5: Complete typing boundary | Type Run phase, control, context and pending; Task status; and related control/transition arguments. Type Context usage and known history records, and Pending model responses, approvals, read plans and selected-media records. Arbitrary arguments/results/human response contents remain JSON inside typed envelopes. | Accepted |
| Q6: Presence and representation | Preserve absence, explicit null and actual values separately wherever legacy data admits them. Preserve existing missing-field defaults without materializing them on save. Validate timestamps while avoiding unnecessary changes to stored string representations. JSON key order/whitespace are outside the compatibility guarantee. | Superseded by removal of backward compatibility |
| Q7: Cross-field validity | Use narrow resume destinations (`Ready`, `Thinking`, `ToolCall`) and failure-terminal destinations (`Failed`, `Cancelled`). Provide typed construction and cleanup operations for approvals/plans; validate required combinations before execution. | Accepted; legacy-omission promise superseded |
| Q8: Boundary decoding and invalid-row isolation | Keep unvalidated forms only inside persistence/transport boundaries. Convert to typed domain values before ordinary execution. Use a lease/revision-fenced invalid-row failure path; a decoding or timestamp failure in one persisted row must not block unrelated work. Reject invalid external state at its receiving boundary. | Accepted |
| Q9: Source versus persisted compatibility | Permit Rust source changes and update call sites rather than keeping duplicate string APIs. Regenerate OpenAPI/web types. | Accepted; persisted/wire compatibility promise superseded |

Round 2 response: `全て推奨`.

## Final round: accepted decisions

| Question | Decision | Status |
| --- | --- | --- |
| Q15: State structure | Use a phase-specific `RunState` enum as the source of truth, derive `RunPhase`, and type waiting reasons/continuations and Context history. Do not add an `Opaque` compatibility variant or unknown-field preservation. | Accepted |
| Q16: Persistence and protocol | Permit JSON/API changes while retaining useful physical columns such as phase TEXT and pending JSONB. Mark the current state format explicitly, update required triggers/indexes through migrations, add no legacy conversion layer, and align OpenAPI/web/Federation. | Accepted |
| Q17: Existing unsupported state | Do not resume old-format Runs. Eligible nonterminal Runs enter a lease-fenced failure path; paused Runs and live leases are respected, and terminal Runs never revive. Keep inspection/stop access and existing Run/journal identity. Do not start replacement execution automatically. | Accepted |

Final-round response: `全て推奨`.

## Consolidation

The decision frontier is empty. [ADR 0011](../adr/0011-strict-typed-run-state.md) and the [specification](2026-10-01-typed-run-state-specification.md) record the accepted model, unsupported-state disposition, implementation sequence, rollout boundary and current-format acceptance matrix. Module names and nested field names remain reversible implementation details within that contract.

This intentionally expands the Issue's original phase/pending scope and replaces its original nonbreaking/unchanged-data acceptance. `RunPhase` is a typed derived value rather than another independently mutable domain field. Valid execution, authorization, recovery fencing and effect semantics remain the behavioral baseline.

This interview records the accepted decisions. Implementation reused its dedicated checkout at `../worktrees/docs/issue-62-typed-run-state-design` and renamed the branch to `feat/issue-62-typed-run-state`, based on the inspected SHA above. The [validation record](2026-10-01-typed-run-state-validation.md) owns implementation and runtime evidence.

## Decision records

- [Superseded ADR 0010](../adr/0010-preserve-run-state-representation.md): records the withdrawn compatibility decision.
- [ADR 0011: Derive Run phase from strict typed execution state](../adr/0011-strict-typed-run-state.md): current architectural decision.
- [Glossary](../../CONTEXT.md): Run state, Run phase, Run control, Task status, Execution context, Pending state and Waiting reason.

## References

- [Serde field attributes](https://serde.rs/field-attrs.html): optional/default behavior is explicitly defined for the current format.
- [Serde flatten](https://serde.rs/attr-flatten.html): consulted for the withdrawn preservation design; the accepted design does not use extension maps.
- [SQLx 0.8.6 JSON adapter](https://docs.rs/sqlx/0.8.6/sqlx/types/struct.Json.html): JSONB can keep its existing database type while Rust uses structured values; direct typed decoding still needs a separate invalid-row strategy.
