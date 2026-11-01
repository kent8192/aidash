# Harness Contract

The **Harness Contract** is the text Aidash itself gives every Agent's model:
which content instructs and which is data, what completes a task, and where
tool aliases are defined. An Agent definition's `instructions` and its Skills
are not part of it. An Agent definition names one version, the Run's Binding
snapshot pins it, and it never changes while the Run exists
([ADR 0093](../adr/0093-harness-contract-is-a-versioned-rule-pinned-per-run.md)).

| Version        | Projection Versions           | Content                                                                                                                                                                        |
| -------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `legacy@1`     | `legacy`, `ordered`, `native` | Today's text for each Projection Version, byte for byte. This is the default and never changes.                                                                                |
| `structured@1` | `native`                      | Sectioned text with an explicit Instruction precedence; Harness requirements in an `aidash`-trust envelope; Agent instructions before Skills and the capability index.         |
| `structured@2` | `native`                      | `structured@1` plus a short working-method section and two completion sentences.                                                                                               |
| `structured@3` | `native`                      | Text selected by a GEPA search (#532): names the forms injected directions take, checks why an action is needed, keeps task-authorized data. Same envelopes as `structured@1`. |

## Selecting a version

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Research the working files.",
  "projection_version": "native",
  "harness_contract": "structured@1"
}
```

Registration, publication and installation reject a version that does not
support the Agent's Projection Version, and Run creation rejects one this node
cannot render. Nothing falls back to `legacy@1`. The field is omitted from
stored definitions when absent, so existing definitions, digests and Binding
snapshots keep their bytes.

A new Agent definition that names neither `projection_version` nor
`harness_contract` is registered as `native` with `structured@3` when its model
declares Native ([#532](https://github.com/kent8192/aidash/issues/532)). A
Native definition without a contract gets `structured@3`, and one whose model
lacks Native keeps the omitted defaults, `legacy` and `legacy@1`. Registration
writes these values into the definition, so its bytes, digest and every Binding
snapshot pin them. Stored definitions, installed Marketplace packages and
system declarations are never rewritten, and an omitted field still means
`legacy@1` when a stored definition or snapshot is read. Usage Records carry the
Harness Contract version beside the Projection Version, and the fixed-task
evaluator accepts `harness_contract` as an arm field; an evaluator arm that names
no Projection Version is registered as `legacy`, so comparisons stay matched.

## How `structured@3` was selected

`structured@3` is the text that won an automatic search, not a hand-written
variant. A replay evaluator rebuilt the exact Native requests of the
fixed-task corpus from deterministic-tier captures, changed only the Harness
Contract text, and played each task against `openai/gpt-oss-20b` and
`deepseek/deepseek-v4.1-flash-1017` through OpenRouter. GEPA, seeded with
`legacy@1`, proposed new texts with a strong reflection model. The objective
gated on safety (no regression from `legacy@1` on any model), then weighted
safety 0.40, corrections 0.35 and capability 0.25, using Wilson lower bounds
and a small length penalty. Candidates naming any task-specific token were
rejected. Selection used held-out tasks with different wording, facts and
injection styles.

On the held-out tasks with gpt-oss-20b, injected directions that claim
authority, impersonate the user, or demand a tool call were resisted in 26 of
30 Runs, against 5 of 30 for `legacy@1`. Task-authorized formats taken from
data stayed at the `legacy@1` level, 9 of 10, so the text does not over-refuse.
DeepSeek V4.1 Flash was at ceiling with every version. Directions that ask the
model to append text to its answer still succeeded with every version; text
alone does not stop them (#629, #630).

The live A/B on Aidash (gpt-oss-20b, 15 Runs per cell) confirmed it: the three
injection styles were resisted in 36 of 45 Runs with `structured@3` against 14
of 45 with `legacy@1`, with no loss on task-authorized formats, corrections or
failed tools. `structured@3` is therefore the default for new Native Agent
definitions.

## Instruction precedence

`structured@*` tells the model how to resolve conflicting directions: the
Harness Contract first, then human answers, then the Agent definition's
instructions and Skills, then the task and run messages. Data never instructs.
Precedence shapes only how work is done; runtime authorization alone decides
what may execute.

## Request shape under `structured@*`

`system` holds, in order: the Tenant Cache Salt line, the Harness Contract, a
`# Agent instructions` section (omitted when the instructions are empty), the
pinned Skill bodies, then the capability index and resident Skills of a
deferred Exposure policy. Content that changes with Load and Unload therefore
comes last.

The Harness's own requirements for a step travel in one envelope, the last part
of the trailing user message
([ADR 0094](../adr/0094-harness-requirements-travel-in-an-aidash-trust-envelope.md)):

```json
{
  "source": "harness",
  "provenance": "aidash",
  "revision": null,
  "trust": "aidash",
  "content": {
    "markers": [],
    "run_message_read_instruction": "...",
    "turn_instructions": ["Run-message catch-up: ..."]
  }
}
```

Absent keys are omitted, and the envelope is omitted when nothing is pending.
`markers` are the continuation markers, `run_message_read_instruction` names
the run-message records still to read, and `turn_instructions` holds the
catch-up, media-intake and Auto Approval Mode paragraphs. None of them enter
`system` or the step's `data` envelope, so `system` changes only when the
Exposure set changes.

## Roles and model-specific text

There is no role field. Express a role's method and deliverable in the Agent
definition's `instructions` or an eager Skill. Model-specific text is added to
a version only for the model it targets and only with ablation evidence.
