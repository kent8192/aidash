# Auto's judge is a Decider the Tenant chooses, and it never sees tool results

Decided on 2026-10-19 for #408 and #412. In the Auto Approval Mode, a model decides each approval instead of a person, as Claude Code's auto mode classifier does. That judge is a Decider: Aidash's decision layer already pins versions, approves each one per Node, tries Decider Routes for availability, runs in Shadow and keeps Decision Evidence. The Tenant chooses the approval Decider version, because it answers for the result, and an Agent's Binding can only make its thresholds stricter, so an Agent's author cannot pick a lenient judge. Aidash ships a built-in approval Decider, bound by default, that each Tenant approves, as with the built-in compaction Decider (ADR 0042), called over the Tenant's OpenRouter connection (ADR 0041).

The Decider's state holds people's inputs in the Thread, the Agent's instructions, the Run's earlier tool calls without their results, and the pending call with its definition. It never holds tool results or the model's own text, so instructions planted in what a Run read cannot reach the judge.

## Considered Options

- **A model binding outside the decision layer**, as the Summary Stage uses. It would duplicate versioning, approval, routing and evidence. Rejected.
- **Let the Agent's Binding choose the judge.** Rejected for the reason above.
- **Pause the Run when the Decider cannot answer**, as compaction does. An approval concerns one call, so refusing it and telling the model keeps the Run going. Rejected.

## Consequences

- A blocked call is refused with a typed reason the model receives. Three blocks in a row, or twenty in one Run, return that Run to asking until a person approves one.
- A Tenant can run the approval Decider in Shadow to compare it with its people's decisions before it decides anything.
