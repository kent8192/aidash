# Matched Projection Version comparison protocol

Design record for [Issue #181](https://github.com/kent8192/aidash/issues/181).
The twelve decisions below were accepted in two design rounds on 2026-10-23.
The final section distinguishes engineering choices from those decisions and
lists execution prerequisites. This document is a design, not an experiment
report or approval to change a default.

The implementation audit used source commit
`37d427e09b519fbede493d01997e2447472126dd`. Actual experiments must identify
their own source commit, binary, corpus, evaluator and configuration. Issue
#122 is evolving; its task set must be identified by an immutable snapshot,
not by an issue number alone.

## Accepted decisions

### 1. Produce evidence sufficient to evaluate a default change

Use the existing eight projection tasks in `fixed-task-corpus-v1` as the
starting minimum. Extend their live execution support, and add or strengthen
coverage for Native multi-call Model Turns, retained corrections and actual
compaction. Freeze the inputs and independent oracles before comparing arms.
Do not remove difficult tasks merely because the current live runner cannot
execute them.

A small pilot may be published as intermediate evidence, but it does not
satisfy the complete default-change gate. Issue #122's broader usability
evaluation remains a separate obligation; this comparison does not claim to
complete it. Scripted fixtures verify accounting, request semantics and the
evaluator. They do not establish provider quality or prices.

### 2. Separate Projection Version effects from explicit caching effects

For each route, keep the task snapshot, model, reasoning configuration,
authority, bindings and other harness settings matched across Projection
Versions. Analyze routes separately.

| Route             | Primary comparison                                       | Additional caching comparison                                  |
| ----------------- | -------------------------------------------------------- | -------------------------------------------------------------- |
| Automatic caching | Legacy, Ordered and Native, all with `prompt_cache: off` | None required; the provider's automatic caching still operates |
| Explicit caching  | Legacy, Ordered and Native, all with `prompt_cache: off` | Ordered off versus explicit, and Native off versus explicit    |

Legacy cannot select `prompt_cache: explicit`. A comparison of Legacy off
against Native explicit therefore measures a combined change and must not be
presented as the isolated effect of a Projection Version. Record actual
serving-route evidence and disclose any failure to match it; a shared
credential catalog provider is not proof of the same upstream route.

### 3. Separate estimator error from estimator-induced execution changes

Fix the selected estimator configuration and calibration conditions in the
primary projection comparison. Compute per-estimator error separately on the
same requests where the estimator is applicable. Do not change the execution
estimator just to obtain another error measurement.

Report Bytes, Calibrated and Tokenized separately, with estimator identity,
version, confidence, encoding and calibration state as applicable. A model
without a supported Token Encoding has no applicable Tokenized result.
Before a default change, also confirm the candidate under the estimator
configuration intended for deployment. Decision 11 selects Bytes for the
primary comparison.

### 4. Measure total provider cost incurred by the task

Report main inference, Summary Stage, Decider and other task-induced provider
work separately, and combine compatible cost units without double-counting.
Retain failed, retried and interrupted work. Reservations, provider-reported
charges and unknown charges are distinct values.

Cost per successful task includes the cost of unsuccessful work in its
numerator. Report its denominator explicitly. Do not present the main
inference Usage Records alone as total task cost when other calls occurred.
Unknown amounts stay unknown, never zero.

### 5. Limit any default change to supported, evaluated new definitions

If evidence supports a change, initially apply it only to new definition
versions created for evaluated, supported model configurations. Persist the
selected Projection Version explicitly at creation. Preserve explicit user
choices, existing definitions, Binding snapshots, Runs and the Legacy meaning
of historically omitted fields.

Do not change `ProjectionVersion::default()` as a shortcut. Native-unsupported
paths, including the current ChatGPT Plan Connection, remain outside the
change. Never silently downgrade an unsupported explicit selection. Document
the exact rollout and rollback controls before enabling the new creation
behavior; rollback affects future creation, not pinned Runs.

### 6. Evaluate quality and safety before efficiency

Evaluate safety violations, false completion and constraint loss separately;
do not average them away in a quality score or economic gain. Compare total
cost and time only for candidates that satisfy the predeclared quality and
safety criteria.

Keep Legacy when required measurements are unknown, the sample is too small,
or the evidence does not support a change. Absence of an observed regression
is not proof of equivalent quality. Fix tolerances, repeat counts, uncertainty
rules and budgets before the experiment used for the switching decision.

### 7. Predeclare the quality and economic gates

For each route, require the confidence bound to support a success-rate
decrease of less than five percentage points and a reduction of at least ten
percent in total provider cost per independently successful task. This is a
claim about the frozen task population, not every task or other models.

Publish task-level results. Judge safety violations, false completion and
constraint loss separately; a weighted average cannot offset their
regressions. Engineering definitions of the eligible completion population
and confidence bounds appear below.

### 8. Separate pilot selection from independent confirmation

Start with ten repetitions per task and experimental arm. Use the pilot to
measure cost, dispersion, missing evidence and feasibility, then freeze the
candidate and confirmation sample size before starting confirmation. Use new
attempts with the same frozen task snapshots for confirmation. Do not reuse
pilot observations as confirmatory evidence or add trials until a test passes.

If the required sample does not fit the budget, report an inconclusive
switching decision and keep Legacy. A pilot is still useful measurement, but
does not by itself establish quality non-inferiority.

### 9. Separate first-attempt cache state from within-Run reuse

Report the first dispatched attempt and subsequent attempts within each Run
separately. Do not call the first attempt cold unless there is independent
evidence that it is cold. Keep reported cache reads, including first-attempt
hits, and keep absent counters unknown. A new Tenant does not clear Legacy's
unsalted provider prefix.

Balance and randomize execution order with a recorded seed. Report deliberate
cross-Run warm-cache trials separately; do not mix them into the primary
condition. Do not modify prompts with nonces and then treat that intervention
as equivalent to normal execution.

### 10. Pin the upstream route without weakening ZDR

Restrict each model to one exact upstream provider endpoint tag, disable
fallback, and preserve ZDR and required-parameter checks. Record the requested
route separately from provider-reported model/provider/endpoint identities.
Do not infer a reported identity from the requested configuration.

Retain attempts with route mismatch or missing route evidence, and identify
their comparability limits. They must not silently become successful matched
pairs. Configured route restriction alone is not a claim that an unreported
backend revision or physical cache placement was observed.

### 11. Use Bytes for the primary execution comparison

Explicitly select Bytes for every primary arm. This prevents accumulated
calibration samples from changing fitting and compaction decisions during the
projection comparison. Compute Calibrated and applicable Tokenized estimates
off the same captured requests using separately pinned inputs.

Before changing a default, additionally confirm the candidate using the
estimator configuration intended for deployment. Keep that evidence distinct
from the primary Bytes comparison and include its work in the shared budget.

### 12. Bound the entire live campaign

The initial total live-provider ceiling is USD 50 and four hours of campaign
execution, shared across preflight, pilot, confirmation and deployment-mode
checks. Model selection and worst-case reservations must fit those limits
before dispatch. Building and deterministic local verification are outside
the live-campaign clock.

Do not quietly remove tasks or required evidence to fit the ceiling. If a
complete phase cannot fit, do not start that phase. Stop dispatch on budget,
deadline, safety failure or unresolved cancellation, retain all attempted and
skipped work, and leave the switching decision inconclusive when necessary.
Agreement on the ceiling alone does not start or authorize paid execution.

## Implementation facts to address

- The [current evaluation framework](../evaluation.md) has a two-arm
  Legacy/Ordered projection comparison. Only two of the eight projection
  tasks are currently live-runnable; Native and actual compaction need
  additional coverage.
- The live runner currently reads accepted Workspace events rather than
  complete Usage Records. It cannot yet provide the required attempt-level
  cache, cost and estimator evidence.
- The Usage Record cache identity covers the selected projection, Tenant,
  key version, instructions and tools, not the whole growing history prefix.
  Legacy has no such identity. Identity changes, actual prefix changes and
  provider cache hits must not be conflated.
- Existing [explicit-cache evidence](../operations/evidence/2026-10-10-prompt-cache-explicit/README.md)
  names a different draft implementation and is not a current three-version
  measurement.
- Existing [projection pinning](../adr/0015-projection-version-pinned-per-run.md),
  [estimator snapshots](../adr/0017-request-estimates-pin-a-calibration-snapshot-per-step.md)
  and [unknown usage accounting](../adr/0018-usage-records-keep-unknown-distinct-and-count-cache-once.md)
  remain binding constraints.

## Engineering choices for implementation

These are the design author's choices within the accepted boundaries, not
additional answers attributed to the user. Freeze their exact implementation
and evaluator version before collecting pilot data.

### Reuse the evaluator and keep complete evidence

Extend the existing fixed-task evaluator rather than creating an unrelated
benchmark. Introduce a new corpus/manifest revision for three-way projection
arms and the additional diagnostics. Preserve all eight existing task IDs in
the report. Add or strengthen Native multi-call, deferred Tool/Skill and
actual pruning/Summary Stage cases with independent oracles. Register any
additional case before the pilot, not in response to favorable results.

The two coding IDs have identical live prompts, inputs and oracles; only the
scripted fixture behavior differs. Keep both rows but give each half the
coding group's weight. The primary completion population has four equally
weighted semantic groups: coding, overdue totals, Human correction retention,
and tool-result correction retention. Thus the five primary task IDs receive
weights 1/8, 1/8, 1/4, 1/4 and 1/4. Additional cases are initially diagnostic
and cannot enlarge the primary population after results are visible.

Terminal overflow, denied approval and step-limit cases remain control rows
with independently defined live acceptance criteria. Do not apply scripted
`tasks.expected` results to real models. Policy-compliant escalation is not
usable task completion. Report control outcomes, their costs and total costs
over the entire campaign separately. For the primary completion population,
include every attempted Run, including failures, in the cost numerator and
success-rate denominator. Report both raw totals and the fixed-weight
estimates; do not count duplicate coding rows as additional task diversity.

Live code oracles run model-produced code only in disposable, resource-bounded
containers with no network and no host secrets. Live fixture tools return
frozen synthetic responses through an allowlisted test service; denied
irreversible actions never dispatch. Equivalent arms use isolated Workspaces
and equal, versioned authority and source seeds.

### Freeze selection and statistical methods

The pilot may select one candidate Projection Version for confirmation across
the two required routes, or select no candidate. An explicit cache policy may
vary by route as decision 2 permits; its selection is frozen before
confirmation. A caching-enabled candidate is labeled as a combined treatment,
not as the isolated projection effect. Actual adoption requires every required
route and invariant gate to pass; it does not support a global all-model
default.

Use one-sided 97.5% bounds for each route's quality and cost gate. Both gates
on both required routes must pass. Report every failure population and every
task separately. Any safety violation or candidate-only false completion or
constraint-loss regression disqualifies the candidate, independently of the
average quality or cost result. These guards are not assertions that unseen
failures have probability zero.

For a conservative finite-sample quality bound, compute the paired gain and
loss probabilities in each of the five primary task strata. Use exact
one-sided Clopper-Pearson lower gain and upper loss bounds, each with error
`0.025 / (2 * 5)`, and weight their differences by the fixed weights above.
The resulting lower bound must exceed `-0.05`. This retains uncertainty even
with no observed discordant pairs; naive Wald or zero-width bootstrap bounds
are not acceptable. The construction assumes independent repetition blocks;
unresolved dependence invalidates the adoption inference.

For cost, form the ratio of fixed-weight expected total cost to fixed-weight
success probability for each arm. Use an enforceable, predeclared per-Run cost
upper bound `B` for each task and arm. For each of the five primary strata,
bound candidate mean cost above, Legacy mean cost below, Legacy success above
and candidate success below, allocating error `0.025 / (4 * 5)` to each
one-sided bound. Use exact Clopper-Pearson for success and bounded
empirical-Bernstein bounds for mean cost. With `m` repetitions, sample
variance `s2` and the allocated error `delta`, the cost radius is
`sqrt(2 * s2 * log(2/delta) / m) + 7 * B * log(2/delta) / (3 * (m-1))`.
Clip mean-cost bounds to `[0, B]` and combine them using the fixed weights.

The upper ratio bound is
`U_cost_candidate * U_success_legacy / (L_cost_legacy * L_success_candidate)`;
it must be below `0.90`. A zero lower denominator gives an infinite bound.
The nonzero range term prevents a zero-width cost interval when every
observed cost is the same. Unknown charges, no enforceable cost bound,
unresolved dependence or non-finite bounds yield inconclusive results, never
a pass. Descriptive block bootstrap intervals may accompany these conservative
adoption bounds but do not replace them after seeing the results. Validate
the calculation and its zero/degenerate cases before the pilot.

Choose confirmation repetitions once from pilot uncertainty and simulation,
targeting 95% power for each of the four route/quality/cost tests under the
declared planning alternatives: quality difference zero and cost ratio 0.80.
These alternatives are not expected findings. Evaluate candidate sample sizes
50 through 250 repetition blocks per task per route in increments of 25,
using 10,000 simulations per size, a fixed seed and the actual bound
calculations. Require the one-sided 95% exact lower bound on each simulated
test's pass probability to reach 0.95. Record the pilot covariance and
conservative discordance sensitivity conditions rather than assuming that
ten observed ties imply no uncertainty. If the pilot cannot support a useful
planning distribution, do not manufacture one to justify dispatch.

Freeze the smallest qualifying sample size before confirmation. At the
250-repeat limit, the eight original tasks require at most 8,000 confirmation
Runs across two routes and two arms, in addition to their 640 pilot Runs;
extra diagnostic cases add separately declared work. The shared money/time
cap takes precedence: if no qualifying complete confirmation fits, report
that limitation without running a smaller sample as if adequately powered.

Method references: [NIST exact binomial intervals](https://itl.nist.gov/div898/handbook/prc/section2/prc241.htm)
and [Maurer and Pontil, Theorem 4](https://www.cs.mcgill.ca/~colt2009/papers/012.pdf).
These are statistical methods; they supply no Aidash performance evidence.

### Candidate routes and configuration

The following are engineering selections from anonymous public catalog,
endpoint and ZDR metadata checked on 2026-10-23. They are candidates for the
execution manifest, not proof of live availability or successful caching.

| Purpose           | Requested model              | Exact endpoint tag     | Input / output / cache-read / cache-write USD per million tokens                |
| ----------------- | ---------------------------- | ---------------------- | ------------------------------------------------------------------------------- |
| Automatic caching | `openai/gpt-oss-20b`         | `groq`                 | 0.075 / 0.30 / 0.0375 / unknown                                                 |
| Explicit caching  | `anthropic/claude-haiku-5.5` | `google-vertex/global` | 0.10 / 0.50 / 0.01 / 0.125 below the endpoint's 100,000-token pricing threshold |

Sources: [automatic endpoints](https://openrouter.ai/api/v1/models/openai/gpt-oss-20b/endpoints),
[explicit endpoints](https://openrouter.ai/api/v1/models/anthropic/claude-haiku-5.5/endpoints),
[ZDR endpoints](https://openrouter.ai/api/v1/endpoints/zdr),
[route restrictions](https://openrouter.ai/docs/guides/routing/provider-selection)
and [prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching).
Archive bounded public metadata and its retrieval time/digest in the execution
manifest; refresh it before paid work. Record the Anthropic canonical slug
`anthropic/claude-haiku-5.5-20261007` separately from the requested alias.

Use `reasoning_effort: medium`, preserve each task's step limit, and initially
preserve the corpus's 128,000/4,096 standard context/output caps and
62,000/1,024 small-window caps. These are experimental caps, not claims about
the provider's maximum window. Keep every such value identical across the
projection arms for that task and route. Pin any compaction Decider, Summary
model, memory models and their allowances before the phase that uses them.

Compute shadow Calibrated errors from a frozen, disjoint calibration sample
set and report the fewer-than-20-samples Bytes fallback explicitly. Tokenized
shadows use an explicitly supported, versioned encoding, initially
`o200k_base` for the gpt-oss route; no Anthropic encoding is presumed.
Compare provider input tokens with the estimate's input side, excluding its
output reservation, and report signed/absolute errors, underestimation and
overflow demotion. Media-free text is the initial scope.

### Export diagnostics without adding a public API

Use an operator-mediated read-only export/import path for live Usage Records,
other provider work, recovery records and route metadata. Extend
`manage diagnostics evaluation` as needed. Its current behavior only emits
SQL; it does not connect to a remote Deployment. Execute exports inside the
authorized Tenant scope and validate the experiment's complete Run/attempt
set, duplicate/missing records and source identity on import.

Keep main-inference Usage Records and Decider/Summary/memory accounting in
their own existing ownership boundaries; join them for the report by durable
Run/attempt references. Do not add cache counters on top of prompt tokens or
add upstream cost a second time to the billed provider amount. Reserve the
worst-case cost of all enabled work before dispatch, not just main inference.

Report cache-identity transitions in attempt order, separately from growing
history/prefix changes and reported hits. Legacy identity is not applicable,
not zero changes. Export only bounded prompt-free metadata; never export
Tenant cache salts, secret material or raw private prompts. Distinguish
provider-rejected Context Overflow, local fitting failures, recovery attempts,
and adopted pruning/Summary Stage operations per Run.

## Execution prerequisites and delivery

1. Implement and validate the extended evaluator, route restrictions, typed
   response identity, complete provider accounting and export/import path.
2. Freeze a clean source commit/binary and the corpus, oracles, evaluator,
   registrations, statistical method, seed and maximum planned work. Run the
   deterministic suite separately from all live evidence.
3. Prepare a concrete live manifest and conservative spend/time estimate for
   the chosen Deployment, Tenant and connected Provider Credential. Verify
   disclosure approval and bounded compaction/model allowances through the
   existing live preflight; do not substitute provider environment keys.
4. Execute only an authorized live plan, retaining uncertain attempts and
   cancellation outcomes. A failed preflight or incomplete phase produces a
   diagnostic report and an explicit unmet gate, not fabricated live evidence.
5. Commit the measurement report and exact reproducible configuration under
   `docs/` as Issue #181 requires. State whether the gate passed, failed or
   remained inconclusive and limit every claim to measured conditions.
6. Only if every required gate passes, prepare the explicit new-definition
   default change with narrow rollout/rollback controls and compatibility
   checks. A no-change result is valid; a design or fixture report alone is
   not completion of the live comparison.

No paid experiment, product implementation, default change or rollout is
authorized by this design record alone. Actual connection details and an
approved concrete live manifest are execution prerequisites, not unanswered
product-design questions.
