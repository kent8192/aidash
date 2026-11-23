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

## Amendment (2026-11-23)

Task-start recall ranks facts that match any literal keyword from the Task title and description.
The Thread owns the bank by default under ADR 0140. Tenant-wide sharing also considers the
same Agent's facts in other authorized Threads. The initial recall payload fits up to eight facts
in 1024 UTF-8 bytes of JSON, including their Unit IDs, revisions and provenance. Each inference
reloads the selected revisions under current authority.
