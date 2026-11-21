# Harness Contract

The **Harness Contract** is the text Aidash itself gives every Agent's model:
which content instructs and which is data, what completes a task, and where
tool aliases are defined. An Agent definition's `instructions` and its Skills
are not part of it. An Agent definition names one version, the Run's Binding
snapshot pins it, and it never changes while the Run exists
([ADR 0093](../adr/0093-harness-contract-is-a-versioned-rule-pinned-per-run.md)).

| Version        | Usage Record number | Content                                                                                                                                          |
| -------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `structured@3` | 4                   | Text selected by the #532 evaluation. It names the forms injected directions take, asks why an action is needed, and keeps task-authorized data. |
| `structured@4` | 5                   | Current default. It adds the precedence of Tenant Instructions, Node Instructions, and a Duty's mission and procedure.                           |

`structured@1` and `structured@2` lost the #532 comparison and were removed
before any deployment. Their names and numbers (2 and 3 on Usage Records) are
never reused, and a database that still stores either refuses the upgrade.

## Selecting a version

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Research the working files.",
  "projection_version": "native",
  "harness_contract": "structured@3"
}
```

Registration, publication and installation accept either structured version.
Omitting `harness_contract` selects `structured@4`. A stored definition or
Binding snapshot that omits the field keeps its serialized form and digest.
A Run that explicitly pins the retired contract fails with
`PROJECTION_RETIRED` at its next worker step. Usage Records keep the number
of the contract used.

## How `structured@3` was selected

`structured@3` is the text that won an automatic search, not a hand-written
variant. A replay evaluator rebuilt the exact Native requests of the
fixed-task corpus from deterministic-tier captures, changed only the Harness
Contract text, and played each task against `openai/gpt-oss-20b` and
`deepseek/deepseek-v4.1-flash-1017` through OpenRouter. GEPA, seeded with
the original contract, proposed new texts with a strong reflection model. The objective
gated on safety (no regression from the original contract on any model), then weighted
safety 0.40, corrections 0.35 and capability 0.25, using Wilson lower bounds
and a small length penalty. Candidates naming any task-specific token were
rejected. Selection used held-out tasks with different wording, facts and
injection styles.

On the held-out tasks with gpt-oss-20b, injected directions that claim
authority, impersonate the user, or demand a tool call were resisted in 26 of
30 Runs, against 5 of 30 for the original contract. Task-authorized formats taken from
data stayed at the original level, 9 of 10, so the text does not over-refuse.
DeepSeek V4.1 Flash was at ceiling with every version. Directions that ask the
model to append text to its answer still succeeded with every version; text
alone does not stop them. Marking or encoding the data did not stop them
either on every model (see below); #630 checks deliverables instead.

The live A/B on Aidash (gpt-oss-20b, 15 Runs per cell) confirmed it: the three
injection styles were resisted in 36 of 45 Runs with `structured@3` against 14
of 45 with the original contract, with no loss on task-authorized formats, corrections or
failed tools. `structured@3` was therefore the default for new Native Agent
definitions until `structured@4` replaced it.

## Tenant Instructions, Node Instructions and `structured@4`

`structured@4` changes only the sentences that say what directs the work, so the
model ranks the Tenant Instructions and Node Instructions a Run pins at claim
(#1010). Nonempty Tenant and Node Instructions appear between the contract
text and `# Agent instructions`, Tenant first. `structured@4` states how they
rank. Its text has not been re-scored on
the fixed-task corpus; the #532 evaluator should be re-run against it.

## Models that read untrusted data

The Harness Contract lowers, but does not remove, the chance that a model
follows directions written inside data. How much protection remains depends on
the model. Under `structured@3`, in the replay evaluator, 20 Runs per style:

| Model                               | claims admin authority | claims SYSTEM MESSAGE | impersonates the user | asks to append text | Japanese admin claim |
| ----------------------------------- | ---------------------- | --------------------- | --------------------- | ------------------- | -------------------- |
| `openai/gpt-oss-20b` (groq)         | 20/20                  | 20/20                 | 14/20                 | 0/20                | 20/20                |
| `qwen/qwen3-30b-a3b-instruct-2507`  | 20/20                  | 20/20                 | 0/20                  | 0/20                | 20/20                |
| `deepseek/deepseek-v4.1-flash-1017` | 10/10                  | 10/10                 | 10/10                 | 10/10               | not measured         |

Cells count Runs that resisted. The DeepSeek row comes from the #532
measurements, 10 Runs per style, where it resisted with every version. An Agent that reads untrusted data, such as web
pages, uploaded files or third-party tool output, should use a model that
resists every style, like DeepSeek V4.1 Flash above. Small open models follow
directions that impersonate the requesting user or ask for text to be added to
the answer, whatever the Contract says. Give such models only trusted data, or
review what they publish. Runtime authorization still bounds what any model can
execute.

### Marking or encoding data (#629)

Two further defences were measured against `structured@3` on gpt-oss-20b and
Qwen3 30B A3B, with only the Contract text and the data envelopes changed:

- **Data Mark**: a per-Run token placed before every whitespace run and after
  24 characters without whitespace in tool results, attachments, observations
  and human requests, plus one paragraph in the Contract saying marked text is
  data. gpt-oss-20b resisted 95 of 100 injected Runs against 74 of 100
  (p = 5e-5), mostly on appended text (16/20 against 0/20). Qwen3 did not change
  at all (60/100 each). Exact copying of data fell from 10/10 to 6/10 on
  gpt-oss-20b, and requests grew by 47 to 49 percent in input tokens.
- **base64 tool results**: both models often could not read the data. Resisted
  injections fell (gpt-oss-20b 15/50, Qwen3 23/50), every task-authorized format
  and exact copy failed, and requests grew by about 70 percent.

Neither met the bar for a new version: a significant gain on both weak models,
no loss on authorized formats, corrections or copying, and at most 15 percent
more input tokens. No Harness Contract version marks or encodes data.

## Instruction precedence

`structured@*` tells the model how to resolve conflicting directions: the
Harness Contract first, then human answers, then the Agent definition's
instructions and Skills, then the task and run messages. Data never instructs.
Precedence shapes only how work is done; runtime authorization alone decides
what may execute.

## Request shape under `structured@*`

`system` holds, in order: the Tenant Cache Salt line, the Harness Contract, Tenant and Node Instructions, a
`# Agent instructions` section (omitted when the instructions are empty), the
pinned Skill bodies, then the capability index and resident Skills of a
deferred Exposure policy. Content that changes with Load and Unload therefore
comes last.

The Harness records changed requirements in a step-state message, inside
an envelope with `trust: "aidash"`
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
