---
status: accepted
---

# Harness requirements travel in an `aidash`-trust envelope, outside `system` and outside data

Decided on 2026-10-26 (#531). On the Native Projection Version every data-derived part is wrapped in a provenance envelope with trust `data`, and the contract says data never instructs. Yet the Harness's own per-step requirements, the continuation markers and `run_message_read_instruction`, rode inside the step's `data` envelope, held together by a prose exception, while the catch-up, media-intake and Auto Approval Mode paragraphs were appended to `system`, cutting the Stable Prefix on those steps.

Under the `structured@*` Harness Contracts (ADR 0093), all of these travel in one envelope with `source: "harness"` and the new trust value `aidash`, the last part of the trailing user message, omitted when empty. The rule becomes exceptionless: trust `data` never instructs, trust `aidash` states Aidash's requirements for this step, and human answers stay the only unwrapped input. `system` changes only when the Exposure set changes. Runtime enforcement is unchanged: catch-up and media intake still restrict the tools they send.

## Considered Options

- **Keep the prose exception.** Rejected: a boundary that depends on the model remembering named fields is weaker than one carried by the envelope itself.
- **Move every Harness requirement into `system`.** Rejected: it would cut the Stable Prefix whenever a marker or paragraph appears, and markers change from step to step.
- **Send Harness requirements unwrapped in the user role.** Rejected: unwrapped user-role text is reserved for human answers, and the model could not tell the two apart.
- **Apply the envelope to Legacy and Ordered.** Rejected: they have no trust envelopes, and they stay on the frozen `legacy@1` text.

## Consequences

- Only `structured@*` Runs on Native emit `Trust::Aidash`; `legacy@1` requests are unchanged.
- An instruction delivered in the user role carries less positional weight than `system`; tool restrictions that matter remain enforced at runtime.
