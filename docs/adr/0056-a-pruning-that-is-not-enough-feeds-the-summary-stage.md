# A pruning that is not enough feeds the Summary Stage, and its Decision Evidence records that

Decided on 2026-10-17 for #108 Part B. When the compaction Decider's pruning does not make the request fit and the Context Policy enables the Summary Stage, the Summary Stage continues from the pruned projection, as ADR 0010's pipeline always did. Moving pruning onto the `DecisionGate` made this a decision whose branches change the Run although the decision alone could not fit the request. Such a decision is recorded with a new `staged` outcome. Its Decision Evidence names a digest of the pruned projection and is committed in the same transaction that adopts the summary (`context.compacted`). If the Summary Stage fails or is not adopted, the decision is recorded as rejected (`insufficient`) and changes nothing. In every case Decision Evidence with an effect is stored atomically with that effect and names the projection it produced. The journal checks the saved context against that digest rather than trusting its caller.

## Considered Options

- **Run the Summary Stage on the unpruned projection.** The rule "a rejected decision never touches the Run" stays literal. But the summary must absorb what pruning would have dropped, so it costs more and fails more often, and the charged Decider answers are wasted. Rejected.
- **Record the decision as rejected and append the summary's adoption as a later outcome.** The evidence would first claim no effect and later gain one, and a crash between the two writes leaves a Run changed by evidence that says it was not. Rejected.
- **Pause with `context_unreducible` whenever pruning alone is not enough.** This breaks ADR 0010's pipeline for every Agent that opted in to the Summary Stage. Rejected.

## Consequences

- Decision Evidence moves to a new version (v3). That version also persists the admission-time Binding restrictions and mode, so a decision without attempts can verify its identity. Undeployed v2 rows are not migrated.
- Replay reproduces the pruned projection's digest, not the summary. The summary keeps its own approval, dependency and adoption checks.
- A Decider in Shadow never produces a `staged` decision; its Run continues as if no Decider were bound.
