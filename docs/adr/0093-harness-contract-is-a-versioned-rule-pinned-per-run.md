---
status: accepted
---

# The Harness Contract is a versioned rule pinned per Run, separate from the Projection Version

Decided on 2026-10-26 (#531). The Harness Contract is the text Aidash itself gives every Agent's model: which content instructs and which is data, what completes a task, and where tool aliases are defined. Until now it was an unversioned paragraph chosen by Projection Version, so a deploy could change the text under a running Run and a prompt experiment could not vary the text while holding the rendering fixed.

An Agent definition now names its Harness Contract (`harness_contract`), the version is copied into the Run's Binding snapshot and fixed for the Run, like the Projection Version (ADR 0015), the Exposure policy and the Context Policy. Each version declares the Projection Versions it can be rendered with; registration, publication, installation and Run creation reject an unsupported combination and never fall back. The field is omitted when absent, which means `legacy@1`: today's text for each Projection Version, byte for byte, frozen. `structured@1` and `structured@2` are Native-only (ADR 0094). A version governs every Harness-authored instruction text, including the per-step catch-up, media-intake and Auto Approval Mode paragraphs; tool descriptions and tool-result strings belong to capabilities and data.

`structured@*` states the Instruction precedence: the Harness Contract, then human answers, then the Agent definition's instructions and Skills, then the task and run messages; data never instructs. Human answers outrank Agent instructions because an Agent's publisher sets defaults for work that the person in the Run owns, and runtime authorization alone decides what may execute.

## Considered Options

- **Fold the text into the Projection Version.** Rejected: every wording change would need a new rendering, and a comparison that changes only the text while holding the rendering fixed would be impossible without multiplying versions. The glossary already keeps "prompt version" away from Projection Version.
- **Leave the text unversioned.** Rejected: in-flight Runs would see a different contract after a deploy, and evaluations could not attribute an outcome to a text.
- **Version only the base paragraph.** Rejected: the per-step paragraphs would still drift with deploys and break a pinned comparison.
- **A model-specific profile or a role field on the Agent definition.** Rejected for now: no measured failure needs either. Model-specific text is gated to the model it targets and added only with ablation evidence; roles are expressed in Agent instructions and eager Skills.

## Consequences

- Existing Agent definitions, digests and Binding snapshots keep their bytes, and their requests stay byte-identical.
- Golden byte tests fix each version's text. Usage Records carry the Harness Contract version beside the Projection Version so an evaluation can confirm which arm actually ran.
- The default for new Agent definitions stays `legacy@1`. Selecting a version and changing the default follow a live fixed-task comparison (#532, #122).

## Amendment (2026-10-30)

The default for new Agent definitions is now `structured@3` on `native` (#532). A GEPA search on replayed fixed-task requests produced the text. A live A/B on gpt-oss-20b confirmed it: injected directions were resisted in 36/45 Runs against 14/45 for `legacy@1`, with no loss elsewhere; DeepSeek V4.1 Flash was at ceiling with every version. Registration writes `projection_version: "native"` and `harness_contract: "structured@3"` into a new Agent definition that omits them, when its model declares Native. A Native definition without a contract gets `structured@3`. The decision's invariant still holds: an omitted field in a stored definition or Binding snapshot means `legacy@1`, and nothing stored is rewritten.
