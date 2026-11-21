# Native is the only Projection Version, and model requests outside Run steps are Model Calls

Decided on 2026-11-21 (#1068). The tabechao benchmark on PR #1014 ran every Agent on Legacy, because its model definition was registered through the API without `projection_versions`, and "omitted means Legacy". Legacy sends no Sticky-routing id, so OpenRouter spread one Run's calls over many providers. The prompt cache hit 13.6 %. A side-by-side probe with the same task and model under strict ZDR settled the cause: Native stayed on one provider and read 91.6 % of its input from cache; Legacy used 11 providers, read nothing and cost eight times as much. Ordered was never a default and is superseded by Native. Keeping three renderers kept a silent fallback that costs money and was never chosen on purpose.

## Decisions

- **Native is the only Projection Version.** Legacy and Ordered are removed, and so is the `legacy@1` Harness Contract. The model definition field `projection_versions` is removed: every model is called Native, and a Keyless model must support OpenAI-compatible tool calls to be registered.
- **Omitted now means Native.** A stored Agent definition or Binding snapshot without `projection_version` or `harness_contract` keeps its bytes and is read as Native with the current structured contract. A Run whose snapshot pins Legacy or Ordered fails at its next step with a typed error; it is not converted. Aidash is undeployed, so no other migration is made.
- **A Model Call is one model request made for a purpose other than a step of a Run:** a compaction summary, a memory role, a Workbench test, a media check or an MCP sampling. It is sent in the Native form (system and user messages), never in a Run's projection, and it carries no Projection Version.
- **Every Inference Attempt has exactly one owner: a Stint or a Model Call.** A retry is a new Inference Attempt of the same owner. A Model Call made during a Run belongs to the Stint awake at the time and counts against that Run's Budgets; one made outside a Run belongs to its Tenant and Node.
- **A Model Call records** its purpose, owner, model version, a digest of what was sent, its outcome, usage and cost, and when it started and ended. It never stores the content, because memory and summaries carry personal data.
- **A Decision Attempt and a Compaction Attempt keep their records** and name the Model Call they used. A Decider's request is not a Model Call; it keeps its Decision Evidence.
- **Visibility.** In-Run Model Calls are listed under each Stint of `GET /api/executions/{id}`; out-of-Run ones are aggregated by purpose in the Tenant's usage view and API.

## Considered Options

- **Keep Legacy and fix only the registration default.** Rejected: any definition registered without the field would still fall back silently, and three renderers stay to maintain.
- **Keep Ordered as a cheaper middle step.** Rejected: nothing chose it by default, and Native shares its Stable Prefix and Retrieval Key reuse.
- **Pin a provider for caching (#1060).** Not needed: the Sticky-routing id keeps a Native Run on one ZDR provider. Choosing providers for trust or quality stays open as its own question.
- **Send auxiliary requests in Native but leave them unrecorded.** Rejected: their cost already escaped `inference_usage` attribution (about 13 % in the benchmark), and a sampling or summary needs the same owner and Budget as the step it serves.
- **Call the record "Call".** Rejected: a Call is a tool call in the glossary (Rejected, Failed, Uncertain and Declined Call).

## Consequences

- Stored definitions keep their digests, but what they mean changes; replaying an old Run is no longer byte-identical.
- Every Run sends the Sticky-routing id and the Tenant Cache Salt, so a node needs a Cache Salt Key to create any Run (ADR 0115 already derives one from `core.secret_key`).
- MCP sampling moves from "an Inference Attempt of the Run" (ADR 0074) to "a Model Call of the Run's Stint".
- Removing old-name shims for saved Agents (`workspace_*` declarations, `max_steps`) is separate work.
