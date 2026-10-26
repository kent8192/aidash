# Harness Contract

The **Harness Contract** is the text Aidash itself gives every Agent's model:
which content instructs and which is data, what completes a task, and where
tool aliases are defined. An Agent definition's `instructions` and its Skills
are not part of it. An Agent definition names one version, the Run's Binding
snapshot pins it, and it never changes while the Run exists
([ADR 0093](../adr/0093-harness-contract-is-a-versioned-rule-pinned-per-run.md)).

| Version        | Projection Versions           | Content                                                                                                                                                                |
| -------------- | ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `legacy@1`     | `legacy`, `ordered`, `native` | Today's text for each Projection Version, byte for byte. This is the default and never changes.                                                                        |
| `structured@1` | `native`                      | Sectioned text with an explicit Instruction precedence; Harness requirements in an `aidash`-trust envelope; Agent instructions before Skills and the capability index. |
| `structured@2` | `native`                      | `structured@1` plus a short working-method section and two completion sentences.                                                                                       |

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

The default for new Agent definitions stays `legacy@1`. Which version becomes
the default is decided by a live fixed-task comparison of `legacy@1`,
`structured@1` and `structured@2` on Native
([#532](https://github.com/kent8192/aidash/issues/532)). Usage Records carry the
Harness Contract version beside the Projection Version, and the fixed-task
evaluator accepts `harness_contract` as an arm field.

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
