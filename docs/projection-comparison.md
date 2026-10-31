# Matched projection comparison

Issue [#181](https://github.com/kent8192/aidash/issues/181) gates a change to the
projection selected by new Agent definition versions. The campaign tooling
implements the [frozen protocol](decisions/projection-comparison-protocol.md)
using the [fixed-task evaluator](evaluation.md). It does not change the default:
**Legacy remains selected, and existing definitions and Runs retain their pins.**

The [2026-10-24 evidence report](evaluation/projection-comparison-2026-10-24.md)
records 88 scripted fixture Runs, their calibration failures and all unmeasured
live-provider metrics. It concludes that the evidence is inconclusive.

The checked-in configuration is
[`tests/evaluation/projection-campaign.toml`](../tests/evaluation/projection-campaign.toml).
A plan is an immutable description of an experiment, not approval to spend.
Planning, importing, reporting and choosing a confirmation sample size make no
provider calls. Only `run` can dispatch live work, after its admission checks.

## Frozen population and routes

The v2 corpus retains the original projection tasks and adds Tool/Skill,
multiple-call and recovery diagnostics; v3 keeps those tasks and corrects
three answer oracles (#595). The v1 and v2 corpora are preserved in
`tests/evaluation/corpus-v1.toml` and `corpus-v2.toml` for historical validation. The pilot
plans ten repetitions per task and arm with a seeded, balanced execution order.
Every task remains in the manifest, including skipped or unavailable work.

| Route       | Matched arms                                                           | Selected runtime estimator | Requested upstream model and endpoint tag            |
| ----------- | ---------------------------------------------------------------------- | -------------------------- | ---------------------------------------------------- |
| `automatic` | Legacy, Ordered, Native; all `prompt_cache: off`                       | Bytes                      | `openai/gpt-oss-20b`, `groq`                         |
| `explicit`  | Legacy, Ordered, Native with `off`; Ordered and Native with `explicit` | Bytes                      | `anthropic/claude-haiku-5.5`, `google-vertex/global` |

These are frozen experiment settings, not an assertion that the routes remain
available. Revalidate exact endpoint and ZDR evidence at execution. A requested
endpoint is distinct from a provider-reported endpoint; an absent reported route
remains unknown and prevents a comparable adoption decision. Do not fill it from
the requested pin or infer an exact tag from a provider's display name.

Models on each route use `reasoning_effort: medium`, with context/output caps
128000/4096 for the standard model and 62000/1024 for the small-window model.
The explicit route maps these to `explicit-cache` and `explicit-small-window`;
it must preserve the small-window task rather than silently widening it.
Per-projection calibrated and tokenized estimates are observational shadows of
the same request. They do not choose a different request, fitting decision or
compaction policy during the primary Bytes experiment.

The primary population has fixed weights:

| Task                                    | Weight |
| --------------------------------------- | -----: |
| `code-change-hidden-regression`         |  0.125 |
| `code-change-verified-fix`              |  0.125 |
| `structured-data-overdue-totals`        |   0.25 |
| `long-conversation-corrections`         |   0.25 |
| `correction-lost-in-pruned-tool-result` |   0.25 |

`terminal-context-overflow`, `approval-escalation` and `step-limit-unresolved`
are controls. They must demonstrate their declared safe behavior and never count
as usable task completions. Added diagnostics do not change the primary weights.
Explicit caching is a separate treatment: an improvement in an explicit arm is
not evidence for the isolated effect of its Projection Version.

## Offline plan

Use Python 3.11 or later, from the intended checkout:

```sh
python3 scripts/evaluate-projections.py plan \
  --output private/evaluation/projection-pilot-plan.json
```

The plan records the exact source state, corpus files and combined digest,
fixed-task evaluator identity, campaign evaluator identity, configuration digest,
all planned Run keys and the plan digest. Outputs are created exclusively and
must not overwrite earlier evidence. A plan made from a dirty checkout is useful
for inspection; live admission requires the exact frozen **clean** checkout and
the same deployed revision. Freeze the deliverable commit before preparing the
live plan. Changing source, corpus, evaluator or approved model configurations
requires a new experiment, not an edit to an existing artifact.

## Live admission and shared limits

No approved execution manifest or connected credential is supplied by the
checked-in configuration. A full live comparison is not established by a
fixture report or by this implementation. In particular, the current Decider
transport has no enforceable upstream monetary ceiling for an individual call.
Declaring `max_nanocredits_per_call` alone does not enforce it. Bounded live
Decider execution is therefore refused before dispatch; the complete recovery
matrix remains blocked until that bound is enforceable. Actual Rust recovery
tests provide separate deterministic evidence and do not substitute for this
live requirement.

Preparing a live campaign requires all of the following facts:

- The exact clean deployed source revision and immutable Registry model
  `id@version` references on both routes. Each route supplies a separate
  deployed-source attestation naming the deployed commit, clean state, binary
  digest and deployed revision. This is an explicit source attestation,
  not automatic verification of the remote binary.
- The Tenant's active connected Provider Credential and its current binding;
  models use that credential, rather than an environment provider key or Plan
  Connection. `AIDASH_EVALUATION_TOKEN` authorizes Deployment Administrator
  inspection of the credential and binding;
  `AIDASH_EVALUATION_SUBJECT_TOKEN` authorizes Tenant Registry, Catalog and Task
  operations. Tokens are read
  from the environment and omitted from command arguments and artifacts.
- Fresh exact-route evidence (`tag`, `source`, `verified_at`, `expires_at`),
  required reasoning and model caps, a fixed estimator configuration, and ZDR
  route validation. Missing or ambiguous exact routes are refused.
- An actual time-bounded disclosure approval naming the corpus, all model
  references, canonical model-configuration digests, disclosure scope and
  positive conservative input, output and cache-write reservation rates.
  No approval or current price is implied by an example or prior report.
- An exact synthetic fixture-service contract for Tool tasks, including
  endpoint, corpus digest and task/alias allowlist. Code oracles additionally
  require a pinned disposable Docker image digest with no network, 128 MiB,
  one CPU, 64 pids and a 20-second timeout.
- Complete, enforceable bounds for any auxiliary provider work and sufficient
  remaining shared spend and time for the **entire** phase on both routes.

The shared campaign ceilings are USD 50 (`50_000_000_000` nanocredits) and
four hours (`14_400` seconds). Preflight, pilot, confirmation and deployment-mode
work share them; they are not fresh allowances per route or per retry. The
persisted state starts the clock at campaign execution. Include any earlier paid
preflight in the same campaign accounting. Do not run separate paid preflight
outside this state and then treat the full ceiling as unused.

Before dispatching either route, `run` prices and reserves both complete route
matrices. Reservations include retries, output allowances, cache-write premiums
and approved auxiliary work. If the complete phase cannot fit, it does not
start, and Legacy stays selected. The child runner also enforces its route
ceiling, time limit and approval expiry before admitting a Run.

The generic live runner executes these Runs sequentially. Each route's
`upper_wall_seconds` must therefore be at least its planned Run count multiplied
by the frozen `run_timeout_seconds`; a shorter declared duration is refused.
The default pilot plans 390 automatic-route Runs and 650 explicit-route Runs:
1040 × 180 seconds is 187200 seconds (52 hours), exceeding the shared four-hour
ceiling before any overhead. **The default complete pilot cannot fit its
conservative time admission bound and does not start paid work.** This is a
planning feasibility result, not a measured provider duration. You may
predeclare a different bounded Run timeout in a new frozen configuration, but
the entire phase, overhead and remaining campaign work must still fit the same
shared limits. Do not understate the bound, reset the state or assume unbounded
parallelism to bypass admission.

Use one state path throughout the campaign:

```sh
python3 scripts/evaluate-projections.py run \
  --plan private/evaluation/projection-pilot-plan.json \
  --execution-manifest private/evaluation/projection-execution.json \
  --state private/evaluation/projection-campaign-state.json \
  --output-dir private/evaluation/projection-pilot
```

This command is an interface example; the live prerequisites above must first be
satisfied. The authorized person preparing the campaign supplies the execution
document, which contains no bearer token or raw provider credential:

| Field                                                    | Meaning                                                                           |
| -------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `plan_sha256`, `approved_plan_sha256`                    | The same exact frozen plan digest                                                 |
| `server_revision`                                        | Exact deployed source commit, equal to the plan's clean commit                    |
| `routes.automatic`, `routes.explicit`                    | Both route execution entries are required                                         |
| Each entry's `base_url`, `tenant`, `provider_credential` | Authorized Node URL, Tenant name and connected credential identifier              |
| `model_refs`                                             | Corpus model names mapped to immutable Registry references                        |
| `models`                                                 | Independently fetched, exact serialized configurations, keyed by those references |
| `prices`                                                 | Approved conservative per-token nanocredit rates, keyed by model reference        |
| `disclosure_approval`                                    | Private local path to that route's exact approval document                        |
| `source_attestation`                                     | Private local path to the exact deployed-source attestation                       |
| `tool_endpoint`                                          | The exact approved synthetic fixture-service endpoint, when needed                |
| `upper_wall_seconds`                                     | Conservative complete-route duration, at least planned Runs × frozen Run timeout  |

Each disclosure approval binds `campaign_plan_sha256`, `corpus_sha256`,
`models`, `model_config_digests`, `source_attestation_sha256`, `prices`, an actual `approver`, aware
`approved_at`/`expires_at` timestamps, `max_spend_nanocredits`, a `disclosure`
statement, and the auxiliary-work contract in `external_compaction`. The live
fixed-task evaluator checks actual fetched Registry configurations against
those digests. An approval for a different configuration cannot be reused.
The source attestation contains exactly `commit` (40 lowercase hexadecimal
characters), `dirty: false`, `dirty_diff_sha256: null`, `binary_sha256`
(64 lowercase hexadecimal characters) and `server_revision` equal to that
commit. The approval binds its canonical JSON digest. A generic live invocation
without this attestation retains unknown remote-source facts; the campaign
requires the complete attestation rather than filling those facts locally.

An interrupted or failed process keeps its entire reservation, including across
restarts. Accepted `model.completed` events omit some failed or interrupted
work and never release headroom. An unresolved reservation or stale state lock
requires manual reconciliation; retrying with a new state file must not reset
spent allowance. Existing phase outputs and duplicate phase/route reservations
are refused rather than silently replacing or adding trials.

## Prompt-free evidence export

The live evaluator has no HTTP endpoint for reading the complete Usage Record
ledger. Whoever runs the Deployment obtains the read-only SQL query document
from the same tested binary:

```sh
cargo run --locked -p aidash-server --bin manage -- diagnostics evaluation \
  > private/evaluation/projection-diagnostic-queries.json
```

This command renders SQL and does not connect to services. Execute those queries
in a read-only transaction under the authorized Tenant scope, using the
[established tenancy settings](../server/migrations/README.md#tenancy-scope-settings).
The queries are parameterless and rely on RLS; do not run them in a Deployment
scope or as a role that bypasses RLS. Select the campaign's complete Run/attempt
inventory from the authorized results. Never export prompts, raw context,
messages, credentials, leases or salts.

The evidence JSON envelope follows
[`projection-evidence.schema.json`](../tests/evaluation/schema/projection-evidence.schema.json).
The importer also applies stricter row allowlists, types, joins and accounting
checks from `scripts/projection_evaluation.py`; JSON Schema alone is insufficient.

| Envelope field        | Required value                                                               |
| --------------------- | ---------------------------------------------------------------------------- |
| `schema_version`      | `1`                                                                          |
| `experiment_id`       | Exact fixed-task manifest experiment identifier                              |
| `manifest_sha256`     | SHA-256 of the canonical **whole result manifest**, including its experiment |
| `source`              | Exact typed copy of `experiment.aidash`                                      |
| `tenant_id`           | Authorized, nonempty Tenant identifier                                       |
| `planned_run_keys`    | Every planned variant × repetition, including skipped Runs                   |
| `run_inventory`       | One `{key, run_id, attempt_ids}` entry per attempted durable Run             |
| Six diagnostic arrays | Complete rows from the tables below, joined to the inventory                 |

`attempt_ids` has all five attempt-table keys even when their lists are empty.
Inference, decision, memory and compaction entries use raw attempt IDs. An
embedding reservation uses `request_id/attempt_id` because multiple ancestor
reservations can refer to one physical attempt. `run_recovery` instead has
exactly one row per attempted Run.

| Array                         | Evidence retained                                                                                                                                                                        |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `inference_usage`             | Main and Summary dispatches, outcomes, request estimates and shadows, reported token components and costs, requested route, reported provider identity and Stable Prefix identity digest |
| `decision_attempts`           | Every Decider call and status; unreported monetary cost stays unknown                                                                                                                    |
| `memory_model_attempts`       | Durable source-Run lineage, reserved amounts, provider usage and policy tariff amounts kept separately                                                                                   |
| `context_compaction_attempts` | Pruning/Summary stage, outcome, reason, before/after estimates and `provider_dispatched`; join actual Summary calls to their Usage Records                                               |
| `generation_embedding_usage`  | Ancestor reservations and unpriced-work markers; these are not additional provider charges                                                                                               |
| `run_recovery`                | Run state and recovery counters, plus independently verified guard annotations when available                                                                                            |

A Summary journal entry can precede authorization or reservation. A new entry
starts with `provider_dispatched: false`; it becomes true atomically with the
durable Usage Record before provider I/O. Only proven false establishes that
no provider call needs accounting. A true entry requires its Summary Usage
Record. Historical null is unknown, not false; without matching provider
usage it cannot establish complete cost. Summary requests use their actual
request Projection Version, rather than inheriting a parent Run's arm label.

Shared memory work lists all `source_run_ids`, appears once in the table and in
each owner's attempt inventory. It is charged once for campaign totals; the
report does not invent a per-Run allocation. Historical missing ownership or
incomplete inventories cannot establish complete accounting.

For illustration, this is a redacted **JSON layout**, not an executable approval
or a valid export of a real experiment. The digest, source, identifiers and
inventories must be constructed from actual evidence; the importer will reject
this placeholder against a real manifest:

```json
{
  "schema_version": 1,
  "experiment_id": "REDACTED",
  "manifest_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
  "source": {},
  "tenant_id": "REDACTED",
  "planned_run_keys": [],
  "run_inventory": [],
  "inference_usage": [],
  "decision_attempts": [],
  "memory_model_attempts": [],
  "context_compaction_attempts": [],
  "generation_embedding_usage": [],
  "run_recovery": []
}
```

Independent guard observations may annotate `run_recovery` with
`guard_evidence_complete`, `safety_violation`, `false_completion`,
`constraint_loss` and, for controls, `control_accepted`. The SQL does not invent
these facts. Missing guard evidence is inconclusive, rather than a clean bill
of safety.

## Report and freeze confirmation

Import the result and its matching evidence export per route:

```sh
python3 scripts/evaluate-projections.py report \
  --plan private/evaluation/projection-pilot-plan.json \
  --execution-manifest private/evaluation/projection-execution.json \
  --result automatic=private/evaluation/projection-pilot/pilot-automatic.json \
  --result explicit=private/evaluation/projection-pilot/pilot-explicit.json \
  --export automatic=private/evaluation/projection-pilot/evidence-automatic.json \
  --export explicit=private/evaluation/projection-pilot/evidence-explicit.json \
  --candidate native --explicit-cache off \
  --output private/evaluation/projection-pilot-analysis.json \
  --markdown private/evaluation/projection-pilot-report.md
```

`--candidate` selects the candidate being explored in a pilot. A confirmation
report uses its frozen candidate. `--explicit-cache explicit` names the combined
projection-and-explicit-cache treatment on the explicit route. The analysis
configuration must bind `plan_sha256`. Post hoc `--analysis-config` accepts
`notes` only; freeze every decision-relevant setting before dispatch. It cannot
replace statistical bounds, independence, deployment readiness, routes,
population, phase, candidate or cache-arm selection.

For statistical bounds, `report --execution-manifest PATH` imports the actual
predispatch-approved execution document. The report derives each per-Run bound
`B` from its exact approved prices and model configurations, checks the approval
digest recorded by the result, and verifies the actual spend ledger's exact
Run reservations and phase ceiling. It does not accept an independently typed
post hoc bound. Omit this evidence and `cost_bounds_enforced` remains false;
the economic gate is inconclusive. A Boolean assertion cannot substitute for
actual route, deployment, guard or provider measurements.

Independent repetition blocks are a separate predeclared assumption. The
default configuration sets `independent_blocks = false` and leaves
`independence_evidence` empty. Record the evidence and assessment in the
configuration before freezing a plan; absent that assessment, the statistical
decision remains inconclusive. Seeded ordering and monetary reservations do
not establish independence of provider or cache state.

The JSON artifact retains configuration and digest, every experiment identity,
all Run metrics, separate route decisions, denominators and unknown counts.
All imported phases, including pilot and deployment-mode work, controls,
diagnostics and unsuccessful work, remain in campaign accounting; they do not
all enter the primary confirmation gates. Supply retained evidence for every
prior campaign phase rather than presenting a favorable subset as the complete
campaign. An unimported phase does not become measured or cost-free.
The Markdown report is derived from it. Without complete evidence exports the
report remains useful for fixture diagnostics but is inconclusive for adoption.
Reported metrics include outcome/quality and variance, each token component or
unknown, provider cost, estimator error without output reservation, within-Run
Stable Prefix identity changes, provider overflow and local fitting failures,
compaction attempts and adopted stages. Legacy's Stable Prefix metric is not
applicable. First dispatches and later dispatches are separate; first does not
mean cold cache.

A pilot chooses one candidate and sample size. It is never confirmation:

```sh
python3 scripts/evaluate-projections.py freeze \
  --plan private/evaluation/projection-pilot-plan.json \
  --pilot-analysis private/evaluation/projection-pilot-analysis.json \
  --candidate native \
  --output private/evaluation/projection-confirmation-plan.json
```

The planner considers 50–250 repetitions in increments of 25 using 10,000
seeded simulations per choice, with conservative power bounds. Unsupported
pilot variance, all ties or no qualifying sample size produce a refusal
artifact, exit code 3 and `keep_legacy: true`. Freeze cannot add headroom to the
shared campaign ledger. Confirmation uses new Runs under the same original
configuration, chosen candidate, fixed cache policy and frozen repetition count.
Do not reuse pilot Runs, expand the sample after looking at confirmation, or
stop early when an estimate appears favorable.

A later intended-estimator confirmation is declared separately:

```sh
python3 scripts/evaluate-projections.py deployment-plan \
  --plan private/evaluation/projection-confirmation-plan.json \
  --estimator-snapshot private/evaluation/intended-estimator-snapshot.json \
  --output private/evaluation/projection-deployment-plan.json
```

It retains the chosen projection, cache policy and sample size, records the
exact intended Bytes or calibrated estimator snapshot and uses independent
Runs. Tokenized deployment execution is not supported by this declaration. Primary execution
remains Bytes. To combine actual prior phases without rerunning them, `report`
accepts `--retained-evidence PATH`; that JSON descriptor names a prior `plan`,
its `results` mapping, its `exports` mapping and, when proving reservation bounds,
`execution_manifest`. For example, this descriptor
has valid JSON syntax and names local evidence paths; the named files must
contain the actual immutable artifacts:

```json
{
  "plan": "private/evaluation/projection-pilot-plan.json",
  "execution_manifest": "private/evaluation/projection-execution.json",
  "results": {
    "automatic": "private/evaluation/projection-pilot/pilot-automatic.json",
    "explicit": "private/evaluation/projection-pilot/pilot-explicit.json"
  },
  "exports": {
    "automatic": "private/evaluation/projection-pilot/evidence-automatic.json",
    "explicit": "private/evaluation/projection-pilot/evidence-explicit.json"
  }
}
```

Keep the original artifacts. A configuration readiness flag
is not deployment-mode evidence.

Quality, independent safety guards and complete accounting precede economics.
Each required route must support the frozen quality margin (degradation below
five percentage points) and at least ten percent lower cost per successful
completion with the protocol's conservative confidence bounds. Unknown costs,
route identities or zero success denominators cannot pass. The bounds apply
only to the frozen task population. They do not establish general model
quality, latency improvements or the absence of unseen failures.

## Default rollout and rollback

This change contains no default switch. Fixture outcomes, a passing script,
or a pilot cannot authorize one. A later default-changing change must cite the
committed live report and exact configuration, satisfy both routes and the
intended-estimator confirmation, and document the evaluated creation paths and
rollout/rollback setting. Existing immutable definitions and Runs must keep
their pinned Projection Version; do not rewrite historical omitted fields to
apply a new default. Until those measured gates are met, rollback is simply to
retain Legacy for new definitions, with no migration of existing Runs.

## Rust construction API compatibility

Persisted JSON additions are optional/defaulted and migrations are additive.
Public Rust structs gain fields, so consumers using explicit struct literals
must update them. Initialize `ModelConfig.provider_route` and
`ModelConfig.estimate_diagnostics` to `None` for unchanged behavior; initialize
`ReportedUsage.provider_identity` to `None` when unreported. Main inference
dispatches use `UsagePurpose::Main`, with `requested_route: None` and an empty
`shadow_estimates` collection when these observations are unavailable. Summary
dispatches use their actual request projection and `UsagePurpose::Summary`.
This source compatibility change is labeled `breaking-change`; it does not
change persisted Projection Version defaults.
