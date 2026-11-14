---
status: accepted
---

# Compaction offloads to the Context Journal, and no model decides what is kept

Decided on 2026-11-10 for #850. A live comparison of 340 Runs on 2026-11-07 found that a compaction Decider did not raise task completion. Jev with `compaction.keep/1`, the built-in configuration, passed 29 % of four recovery tasks. Recency with `compaction.fit/1` passed 69 %, Jev with `fit/1` 63 %, Clef with `fit/1` 61 % and a fixed rule 54 %. The same window on the `calibrated` estimator passed 11 of 12 Runs without compacting at all, because `bytes` counted about 4.6 times the provider-reported tokens. No surveyed harness defaults to an external pruning judge, and summaries alone lose recall that a retrievable original keeps (MemGPT, Hermes, LongMemEval).

We removed compaction from the decision layer and rebuilt it in four layers. Nothing leaves the Context Journal:

1. On arrival, a tool result too large for its share of the window becomes an Offloaded Result.
2. When a request does not fit, the oldest tool results become Offloaded Results in one batch, down to about 70 % of the window. The protected recent tail is bounded by size as well as count, and the newest Model Turn is always kept.
3. If that is not enough, the Summary Stage folds older events into the Execution Summary with the Agent's own model.
4. The Agent reads any offloaded content back with Journal Query, read-only code it writes against its own Run's Context Journal.

Each marker names the Context Journal range of its original. The Run pauses only when nothing makes the request fit. New model definitions default to the `calibrated` estimator, which keeps the byte bound until it has enough samples.

## Considered Options

- **Keep a compaction Decider, with `fit/1` as the built-in rule.** On `fit/1` it matched recency and did not beat it, and it costs a Catalog Approval, an OpenRouter binding and a disclosure for every Tenant. Rejected.
- **Summary as the main layer, as Claude Code and Codex do.** Summaries lose details that a later step needs, and the strict validation of ADR 0010 paused 19 of 35 summary-only Runs. Rejected as the main layer; kept as the third layer.
- **Hindsight-style fact extraction as the store compaction reads back from.** Extraction drops the mid-page sentence and the identifier that recovery tasks need, and its measurements cover conversational QA only. Rejected for reading back within a Run; Built-in Memory (ADR 0126) keeps cross-Run facts.
- **Let a model call itself from inside Journal Query, as RLM does.** Deferred until a measurement shows the need. A Subnode Task may instead carry a Journal range for members to search.

## Consequences

- Superseded: the compaction parts of ADR 0042 (built-in compaction Decider), 0056 (`staged` outcome), 0066 (Routes for compaction), 0102 (start check), 0105 (`fit/1`) and 0107 (compaction builder). The approval Decider, the Auto judge (ADR 0077), Decider Routes and Decision Evidence for them are unchanged.
- ADR 0010 changes in three places. The Summary Stage uses the Agent's own model instead of a separately approved summarizer, because the Context Journal stays readable and the content goes nowhere new. Goal, people's words and tool outcomes are copied from the Context Journal rather than written by the model. An unsupported item is dropped instead of rejecting the summary, although dropping an item of the previous summary still rejects it.
- Journal Query runs on runner/1 without network, writes or tool calls, in a kernel apart from `code_interpreter` and keyed to the Run. It needs no Catalog Approval because it reads only the Run's own Context Journal, with revoked content replaced (ADR 0046). Its output reaches the model as data. Without a Runner, a code-free fallback offers search and range reads. Reading the same range back repeatedly sends it to the Summary Stage instead of starting a loop.
- The summary prompt and marker wording are optimized offline with DSPy/GEPA on a panel of model families. Each candidate is scored on the worst family with probes generated from the Context Journal. Aidash runs on any OpenRouter model, so it cannot optimize per model.
- Aidash is undeployed: compaction Decider records and approvals are removed, not migrated.
