---
status: accepted
---

# Every agent has a Built-in Memory on its own model, admitted without a person

Decided on 2026-11-10 for #852. Native memory needed an explicit Binding, several role definitions each with a Catalog Approval, a Workspace semantic index and a recovery ledger. Its failure paused the Run (ADR 0059). In practice no agent had memory, and Run-learned facts waited in a review inbox that unattended Runs never empty (#141).

Every agent now has Built-in Memory as tools to retain, recall and reflect. Extraction and reflection use the agent's own model and Provider Credential. Search is local, by keyword, time and links, so content goes nowhere new and no approval is needed. Semantic search joins only when the Tenant approves an embedding.

Recall enters the context once, at Run start, from the Task text. When compaction offloads results, facts are extracted from them in the background into a Run-scoped memory, which moves to the cross-Run memory when the Run ends. Admission is automatic, with no review and no unreviewed marker. Contradictions are reconciled into observations, as upstream Hindsight does.

Each fact names its Run and Context Journal range. A later Run reads details back from that range under its own authority. What memory recalls reaches the model as data. Memory is shared within a Workspace by default, and a Tenant setting widens it to the agent across the Tenant.

## Considered Options

- **Keep human review of Run-learned facts (the port's deviation from upstream).** Unattended Runs leave candidates to expire, so nothing is learned. Rejected. Injected text is contained instead by provenance and by returning recall as data.
- **Inject recall into every inference.** Hindsight's own sde-bench found per-prompt recall worse than no memory, and it costs a lookup per step. Rejected in favour of once at Run start.
- **Register each completed Run's history as an Artifact.** Too heavy for every Run. The Context Journal already lives as long as its Run, so facts point into it instead.
- **Use extraction as compaction's store for reading back within a Run.** Rejected; see ADR 0125.

## Consequences

- A Built-in Memory failure is a tool error returned to the model. ADR 0059's pause remains for memory a Tenant binds explicitly.
- Built-in Memory must work without a Workspace semantic index and without a recovery ledger, or #852's PR names where it does not. Automatic admission needs a default cap and eviction, because recall loads the whole bank.
- After a Run's Context Journal expires, only its extracted facts remain.

## Amendment (2026-11-17)

A Run that a Remote Grant started recalls and writes memory through its Home (ADR 0139). Recall reads the Home Workspace's published bank and the Agent's participant bank there, as the Run's starting context already did, and writes go only to the participant bank, so nothing the Run learns stays on the receiving Node and it never writes the Home's shared memory directly.

## Amendment (2026-11-25): a person's deletion marks what was learned from it (#928)

When a person deletes a fact, the facts learned from Task executions that recalled it are marked, not withdrawn. The mark spreads to facts learned from Task executions that recalled a marked fact. Recall returns a marked fact with the deleted fact's text and asks the model to rely on it only if it does not depend on that fact. Marked facts rank below unmarked ones. A new revision with new evidence clears the mark. Running Task executions that read the deleted or a newly marked fact are not stopped: they receive a platform notice before their next inference. The memory's own evictions, corrections and reconciliations still change nothing downstream.

A live comparison chose this over withdrawing the facts. DeepSeek V4.1 Flash, with the learned facts recorded by hand and the Agent told to consult memory, gave these results:

- **Nothing re-checkable.** Withdrawing every affected fact removed every poisoned answer (8 of 10 to 0) but also every true fact the same Task executions had learned (9 of 10 to 0). A mark without the deleted text did no better: the model distrusted all marked facts (0 of 8 kept). A mark quoting the deleted fact removed the poisoned answers (11 of 12 to 0) and kept the unrelated facts (11 of 12 kept, against 11 of 12 with no cascade).
- **Source re-checkable.** No arm gave poisoned answers.

### Considered Options

- **No cascade.** Poisoned facts keep answering when nothing can be re-checked.
- **Withdraw facts one hop away.** Poisoned facts two hops away survived (4 of 5 invoice answers poisoned), and the true facts of the first hop were lost.
- **Withdraw transitively.** Removes the poison, but also every unrelated fact the same Task executions learned.
- **Stop Task executions that read the deleted fact.** Built-in Memory failures are tool errors, not stops (above). A notice lets the execution re-check its own work.
