# Oversized context is recovered by a staged, opt-in pipeline over an authoritative Context Journal

Decided on 2026-10-09 for #113. A Run's provider context is a lossy Context Projection of an append-only Context Journal. When the projection does not fit, a versioned Context Policy, frozen into the Run's binding snapshot, runs a fixed pipeline: fit check, Jev pruning, an optional Summary Stage, and complete-request validation. If the request still cannot fit, the Run pauses with a typed reason. A provider-reported Context Overflow re-enters the same pipeline with a smaller effective window, a bounded number of times. Agent versions without a Context Policy keep today's prune-only behavior, and provider overflow then only pauses them.

## Considered Options

- Let the Summary Stage stand in for Jev when Jev is unavailable. Rejected: summary text is generation, and Jev's fallback is an ordered list of approved Decision APIs (#153). A generation model silently replacing the decision layer breaks #103's separation of layers.
- Summarize with the Agent's own model, following `run_message_summary`. Rejected: summary text needs its own exact-version approval, disclosure checks and allowance. The summarizer is a separately pinned model binding with no fallback.
- Reconstruct original history from `invocations`, human requests and run messages. Rejected: human, media and marker events cannot be fully recovered, and stored tool results can differ from the context result. Only an explicit journal makes "the original stays available" testable.
- Parse the provider's real window out of the overflow error body. Rejected: the body format is fragile, and Aidash already reduces upstream bodies to generic text. Overflow only shrinks the effective window by a fixed factor.
- Retry output truncation with a larger output allowance. Rejected: truncation and refusal are deterministic for a given request, so retrying wastes tokens. Both pause with typed reasons and never dispatch their tool calls.
- Keep the never-written `Context.summary` field for compatibility. Rejected during 0.1.0 development: a data migration drops it, while the request JSON keeps its `"summary"` key so prune-only requests stay byte-identical.

## Consequences

- `ContextEvent`s carry a durable `seq` and are appended to `run_context_events` in the same lease-fenced transaction as `save_run`. Runs that existed before the upgrade import their current projection as `imported`, which records that the journal is lossy before that point. The journal's retention matches the Run's, and it is not exposed through the public API.
- The Summary Stage only absorbs `Tool` and `ModelMediaObservation` events. They must be outside the protected recent tail (at least 6 events) and must already have been sent in an accepted inference. `Human` and marker events always stay verbatim, so corrections and continuation metadata cannot be summarized away, and an unread source never turns into a read one.
- An `ExecutionSummary` merges the previous summary instead of resummarizing from scratch. It records its exact journal range and digest, and the union of its source dependencies. It is adopted only if it validates: schema, a non-empty goal, every previous constraint and unresolved item either carried forward or explicitly resolved, within its size bound, and a strict size reduction. On resume after revocation, a summary whose dependencies are no longer valid is discarded and rebuilt from the journal.
- Every summary attempt is recorded in `context_compaction_attempts` before I/O and is adopted atomically with the Run's save, after the lease, journal range and authority are rechecked. Interrupted attempts become `abandoned`, and the saved context stays unchanged.
- Remote RequiredHome execution needs both a `summarizer` pin and a `summary` allowance. Generated chains charge `Purpose::Summary` to every ancestor before I/O.
- OpenRouter requests set `transforms: []`, so the provider never silently drops the middle of a context.
- `InspectionContext` replaces `summary` with `execution_summary`. This is a breaking API change.
