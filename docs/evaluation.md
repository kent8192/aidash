# Fixed-task evaluation

`scripts/evaluate-fixed-tasks.py` measures whether a harness change improves
task outcomes and cost under matched task and model conditions (#122). It runs
a small versioned task corpus, grades each Run with an independent oracle, and
writes a validated result manifest. It complements, and never replaces, the
deterministic protocol, authorization and recovery tests.

Results are measurements. Every manifest sets `grade: "measurement-only"` and
all `approvals` to `false`. Conclusions are limited to the tested tasks, the
tested model or fixture configuration and the recorded repeat counts. A
manifest does not approve a default rollout, a cost claim, or a claim that
Aidash outperforms another harness.

## Tiers

| Tier            | Model                                           | Credentials                                                      | Spend                         | Use                                                            |
| --------------- | ----------------------------------------------- | ---------------------------------------------------------------- | ----------------------------- | -------------------------------------------------------------- |
| `deterministic` | Scripted fixture provider on `127.0.0.1`        | None                                                             | None                          | Local and CI. Checks the harness and the evaluator itself.     |
| `live`          | A model the evaluator registered on a real Node | The Tenant's connected Provider Credential (OAuth, #158), opt-in | Hard ceiling, approved prices | Baseline reports before a default rollout. Never run by PR CI. |

### The one command (deterministic)

```sh
python3 scripts/evaluate-fixed-tasks.py deterministic
```

It needs Docker and Cargo and nothing else: no credentials and no paid calls.
Every Run, the scripted provider and the Node stay on `127.0.0.1`; the only
other network use is the tooling's own, namely pulling the NATS image and
building the PostgreSQL image from `compose.yaml` when they are missing, and
Cargo fetching crates for the first build. It

1. builds this worktree's `manage` binary and records its SHA-256, the commit
   and the dirty state;
2. starts disposable PostgreSQL (`aidash-postgres` image) and NATS containers
   named `aidash-fte-<hex>-*`, removed on exit even after a failure;
3. runs `manage migrate` and `manage memory-recovery init-if-missing` on a
   private recovery directory, then starts one Node in the `serve` role (HTTP
   plus four worker slots that consume activation) with
   `AIDASH_ACTIVATION_BOOTSTRAP=true`, a random Deployment Administrator token and a random
   Cache Salt Key, so Ordered Runs are possible;
4. serves the scripted provider (`scripts/fixed_task_fixture.py`): the model at
   `/v1/chat/completions` and the fixture tools at `/tools/{task}/{alias}`
   (there is no Jev classifier: the fixture Agents bind no compaction Decider);
5. creates a Tenant, a subject token and one Agent definition per task arm,
   runs every task arm `--repeats` times (default 1) as Tenant-scoped Runs,
   up to `--parallel` (default 4) at a time, reads each Run's Usage Records from
   the disposable database with the read-only `manage diagnostics evaluation`
   query, and executes the memory scenarios;
6. writes `private/evaluation/deterministic-<hex>.json` (mode 0600, created
   exclusively) and prints a summary.

The command exits non-zero when the manifest is invalid, when any Run differs
from its task's declared calibration (`[tasks.expected]`), when a
deterministic memory scenario fails, or when the Node rejects the Agent
definition of an arm the corpus marks available. The last would otherwise turn
a harness regression into a quiet `feature-not-available`; pass
`--allow-unavailable-arms` only for a baseline Node that predates the feature
(the #102 baseline). The manifest is written in every case, including after an
error or Ctrl-C, with the failure in its `error` field. Server logs stay in
`private/evaluation/logs-<hex>/`.

Useful options: `--task ID` and `--comparison ID` (repeatable) narrow the plan,
`--binary PATH` uses a built `manage` (by default the command builds this
worktree's; either way the executable is copied into a private directory
first, and the manifest fingerprints and runs that copy), `--run-timeout SECONDS` (default 180,
finite and at least 1 in both tiers) bounds each Run, `--skip-memory` reports
every memory scenario as `not-run`, `--skip-rust-evidence` skips the
`deterministic-test` memory scenario's `cargo test`, and `--evidence-timeout
SECONDS` (default 1800) bounds that command; a timed-out command is killed
with its process group and the scenario is recorded as `error`. The output path is
reserved before setup; when container startup, migration or the health wait
fails, the manifest is still written with the error, so a retry with the same
`--output` is refused only when a real record exists. Ctrl-C and SIGTERM
cancel the queued Runs, cancel the active ones on the Node, remove the owned
Node and containers and write the manifest with every finished or interrupted
Run, listing every Run that was not recorded in `skipped_runs` (exit 143 for SIGTERM); a second SIGTERM while that unwind hangs removes
the owned Node and containers and exits at once. The checkout and executable
must stay as recorded: the `cargo test` evidence commands run only while they
do, and a change found before the manifest is written is recorded as an error.
The recorded source fingerprint covers the diff, the status and the bytes of
every untracked, unignored file. Each Run's Usage Record read is bounded to 60
seconds; a stalled read is recorded as that Run's infrastructure failure.

Wall time depends heavily on host load. Long-context tasks take about twenty
steps each (a human answer, eight reference pages and their approvals). On an
idle workstation the whole corpus takes minutes; on a host shared with many
concurrent builds a single step took up to 100 seconds, so raise
`--run-timeout` there.

`python3 scripts/evaluate-fixed-tasks.py validate FILE` revalidates a manifest
against the corpora it used (`--corpus` and `--memory-corpus`, default the
repository's; their SHA-256 must match the manifest).

### The scripted fixture Agent

The deterministic tier's "model" is a probe. For each request it finds the
registered Task UUID, decodes every string the harness delivered and plays the
task's `fixture_agent` behavior using only that content. When compaction drops
a correction, the probe cannot see it and answers with the stale value, which
the oracle then catches. Its token counts (one token per byte), synthetic
prices and simulated prefix cache are deterministic; they test accounting,
request shape and Usage Records, not provider prices. Reporting at least one
token per byte keeps a calibrated Request Estimate (ADR 0017, capped at the
byte estimate) equal to the conservative one, so compaction decisions do not
drift as Usage Records accumulate. Memory embeddings are 8-dimension hash
vectors with no semantic similarity. The fixture Agents bind no compaction
Decider and have no Summary Stage, so a long-context task stops with
`context_unreducible` when it outgrows its window. The provider rejects
a request larger than the model's declared window with an OpenRouter-style
context-length error.

### Live tier

```sh
AIDASH_EVALUATION_TOKEN=... AIDASH_EVALUATION_SUBJECT_TOKEN=... \
python3 scripts/evaluate-fixed-tasks.py live \
  --base-url https://node.example --tenant TENANT \
  --provider-credential PROVIDER_CREDENTIAL_ID \
  --model standard=MODEL_ID@VERSION \
  --task structured-data-overdue-totals --task deferred-skill-selection \
  --task approval-escalation --comparison projection --comparison exposure \
  --disclosure-approval approval.json \
  --max-spend-nanocredits 500000000 --max-wall-seconds 1800 \
  --server-revision DEPLOYED_SOURCE_FINGERPRINT
```

`--server-revision` is required and must be 7 to 64 lowercase hexadecimal
characters (a git commit or a SHA-256): a live manifest has no local build, so
this is the only identification of the tested source. Every `--task` and
`--comparison` must name a corpus entry and select at least one runnable pair,
also after pairs whose corpus model has no `--model` mapping are dropped; a
misspelled, inapplicable or unmapped selector is refused rather than silently
ignored.

Before it registers anything or starts a Run, the live tier refuses unless:

- `--base-url` is `https://`; plain `http://` is accepted only for a loopback
  Node (`localhost`, `127.0.0.1`, `::1`), because both bearer tokens are sent
  with every request. Redirects are never followed: urllib would resend the
  bearer token to the new location, so a 30x response is a failed request.
  Plain `http://` never goes through an environment proxy (`HTTP_PROXY`),
  which would otherwise receive both tokens in plaintext;
- both bearer tokens are set (Deployment Administrator for Registry writes, Tenant subject for
  Tasks); neither is copied into the manifest;
- the named Provider Credential exists, is `active`, and is the credential the
  Tenant's Provider Credential Binding for its provider names (Run admission
  pins the bound credential, not the one on the command line), the binding has
  a positive `revision`, and every mapped model uses `provider_credential` for
  that provider rather than `credential_env`;
- the disclosure approval document names an approver, approval and expiry
  times, the exact corpus SHA-256, exactly the mapped models, a disclosure
  statement, a maximum spend at least the requested ceiling, positive
  per-token input, cache-write and output prices for each model, and
  `external_compaction: "disabled"`. Every field is type-checked. Times are
  ISO 8601 with a UTC offset (`Z` or `+09:00`) and are compared as instants,
  not strings; the approval must already be in effect (`approved_at` not in
  the future), expire after it was approved, and not have expired;
- every mapped model configuration has a positive `context_window` and
  `max_output_tokens` and a `model_id`;
- a positive spend ceiling, a finite wall-time limit of at least one second
  and a finite `--run-timeout` of at least one second are given (both tiers
  refuse a shorter `--run-timeout`: the manifest records limits in seconds of
  at least one);
- no selected task needs a fixture tool result. Fixture tools point at an
  unroutable endpoint and their approvals are denied, so such a task could
  only fail or pass by guessing; only `approval-escalation`, which needs the
  denial, keeps its tool;
- every selected task's oracle names the fields its kind reads, with valid
  types and regular expressions; the corpus is checked when it loads, so no
  Run is dispatched for a task that cannot be graded;
- the `--output` path (default `private/evaluation/live-<prefix>.json`) can be
  created exclusively. It is reserved before the first Registry write, so a
  path held by another invocation is refused before any paid work;
- no selected task uses a `python-tests` oracle. **The live tier never runs
  model-written code.** Those oracles execute the answer on the evaluator host,
  and nothing there isolates them: a stripped environment does not hide the
  parent's tokens (`ps eww $PPID` on macOS, `/proc/<ppid>/environ` on Linux),
  the child has network access, and a model-written module can exit 0 on its
  own (`os._exit(0)`) so that the held-out tests appear to pass. They need a
  container runner (`--network none`, no host environment, no parent process
  visibility), which is a deferred follow-up. Select other tasks with `--task`.
  The deterministic tier still runs them, because its answers are scripted.

```json
{
  "approver": "evaluator@example",
  "approved_at": "2026-10-11T00:00:00Z",
  "expires_at": "2026-10-12T00:00:00Z",
  "corpus_sha256": "<corpus.sha256 from a deterministic manifest>",
  "models": ["MODEL_ID@VERSION"],
  "max_spend_nanocredits": 500000000,
  "disclosure": "Synthetic corpus only; no private repository content.",
  "external_compaction": "disabled",
  "prices": {
    "MODEL_ID@VERSION": {
      "input_nanocredits_per_token": 100,
      "cache_write_nanocredits_per_token": 125,
      "output_nanocredits_per_token": 500
    }
  }
}
```

During the run, `SpendLedger` reserves each Run's worst case before dispatch:
max steps × six provider attempts per step (the harness retries a retryable
provider failure up to five times without advancing the step) × (the full
context window at the dearer of the input and cache-write prices plus the
output reservation). It refuses any Run that would cross the ceiling, start
at or after the wall-time limit, or start after the approval has expired, and
no Run's own time limit reaches past either. The approval's expiry is read
against the wall clock once, when the ledger is created, and then held on the
monotonic clock, so a later wall-clock correction can neither extend nor
shorten it. The refusal is recorded in
`spend.refusal` and the command exits 3. Planned Runs that were never
dispatched are listed in `skipped_runs` with the reason, so none is silently
dropped.

`external_compaction: "disabled"` attests that the evaluated Tenant approves
no compaction Decider. Only a Run bound to an approved compaction Decider calls
Jev, on the Tenant's own OpenRouter Provider Credential (ADR 0041, ADR 0042). Those calls have no price in the approval and
no HTTP surface that reports the Tenant's approvals, so the live tier cannot
reserve them. A Run whose context records an applied compaction proves a
compaction Decider answered: the manifest records the error and no further Run is
dispatched.

Run admission pins the Tenant's binding inside the claim, and the claim API
takes no expected credential. The evaluator therefore reads the binding just
before each reservation and again once the claim has admitted the Run. The
binding `revision` only grows, so the same credential and revision on both
sides prove the Run pinned the approved credential. Any other result cancels
the admitted Run, records it as an `infrastructure_failure` and stops all
further dispatch.

Every dispatched Run, completed or not, stays charged at its **full
reservation** as uncertain. `model.completed` events are emitted only for
accepted attempts, so interrupted and retried attempts inside a Run would
otherwise be uncharged and release headroom under the hard ceiling. The
accepted attempts the Node reported are kept in
`spend.attempts[].provider_work` for information only (step and token counts).
Pricing Runs from Usage Records waits for an HTTP read endpoint for them.

The live manifest records `model.reported_identities: null`: neither Usage
Records (unreadable over HTTP) nor `model.completed` events carry the
provider-reported model or route, so the manifest cannot say what served a
request. `model.route` is only the connected Provider Credential's catalog
provider. Every request is bounded by the Run's time limit; when it passes,
even inside a stalled request, the Run is cancelled on the Node, and so it is
when a request about the Run fails. A claim retried after a gateway error
may already have admitted the Run, so when its backoff would end past the
time limit the evaluator cancels the Run at once instead of sleeping. A
cancellation the Tenant subject token cannot make is retried once with the
Deployment Administrator token; if that fails too, the Run may still be active, so the
manifest records it and no further Run is dispatched. A claim sent without a
definite answer may admit a Run that appears later: cancellation polls for it
through the 30-second grace period, and a Run that never appears counts as an
unresolved cancellation, which also stops dispatch. Polling never sleeps
across a Run's time limit, and when cancellation after a failure returns the
Run's final state, that state (compactions included) is recorded and checked.
After any failure past the guard rails,
including Ctrl-C and SIGTERM, the in-flight Run is cancelled and the manifest
is still written with the error, the ledger and every finished Run; the exit
code is 130 for Ctrl-C, 143 for SIGTERM, 1 for an error and 3 for a ceiling
refusal. Once dispatch has ended, SIGTERM is ignored and Ctrl-C is deferred
while the Runs are graded and the manifest is written; a deferred Ctrl-C
still exits 130 after the manifest is written.

Live answers are restricted content: the manifest keeps their digests and the
verifier results, not the text. Tool-call arguments and every value a verifier
check observed are kept only as `arguments_sha256` and `observed_sha256`,
because a model can copy task data into them. For the same reason a tool name
other than the Agent's own tools and the builtins is kept only as
`name_sha256`, with the Node's error that echoes it as `error_sha256`, and
`tool_selection` lists it as `sha256:<digest>`. Fixture tools point at an unroutable endpoint
and their approvals are denied, so a live Run never causes an external tool
effect. Usage Records have no HTTP read endpoint, so live token counts come from
Workspace events (at most the latest 100) and do not price anything. The live
tier has been exercised only against in-memory fakes; it has never been run
against a provider.

## Corpus

`tests/evaluation/` is versioned with the code:

| Path                   | Content                                                                               |
| ---------------------- | ------------------------------------------------------------------------------------- |
| `corpus.toml`          | Tasks, fixture models, fixture tools and paired comparisons (`fixed-task-corpus-v1`). |
| `memory.toml`          | The memory matrix corpus (`memory-corpus-v1`).                                        |
| `snapshots/`           | Controlled task inputs. They are embedded in the Task text.                           |
| `oracles/`             | Held-out checks. They are never sent to an Agent.                                     |
| `schema/*.schema.json` | JSON Schemas of the experiment and result manifests.                                  |

The corpus SHA-256 covers `corpus.toml` and every input and oracle file it
references. The memory corpus has its own SHA-256. Changing a task, an input,
an oracle or the evaluator (`evaluator.sha256` covers the four scripts and the
two manifest schemas) changes the manifest's identity, and direct comparisons
with earlier manifests are no longer valid. Each invocation reads the corpus
files once: Task text and oracles use exactly the bytes that were hashed. Both
tiers pin the evaluator identity, and the schemas validation uses, when they
start, so editing an evaluator file during a run changes neither.

### Tasks

| Task                                    | Oracle                                                                           | Deterministic calibration                                |
| --------------------------------------- | -------------------------------------------------------------------------------- | -------------------------------------------------------- |
| `code-change-hidden-regression`         | Visible tests plus held-out `oracles/pricing_hidden.py`                          | `false_completion`: claims success, fails held-out check |
| `code-change-verified-fix`              | Same                                                                             | `success`                                                |
| `structured-data-overdue-totals`        | Totals recomputed from `invoices.csv` by the oracle                              | `success`                                                |
| `long-conversation-corrections`         | Budget and exclusions after corrections                                          | `unresolved`, `none` (not pruned until #108)             |
| `correction-lost-in-pruned-tool-result` | Same                                                                             | `unresolved`, `none` (not pruned until #108)             |
| `terminal-context-overflow`             | Same                                                                             | `unresolved`, `none` (not pruned until #108)             |
| `deferred-tool-selection`               | First domain tool call and its arguments from the Run's history, plus the answer | `success`, selection correct in both arms                |
| `deferred-skill-selection`              | Skill-only fact, no forbidden content; under `deferred@1`, a load of the Skill   | `success` in both arms                                   |
| `approval-escalation`                   | Denied approval for the forbidden action, no dispatch, escalation reported       | `policy_compliant_escalation`                            |
| `step-limit-unresolved`                 | Answer pattern                                                                   | `unresolved` at the step limit                           |

The oracle never reads the Agent's claim. The claim is recorded separately as
`claimed_success` so that false completions are visible; a success phrase
counts unless a negation precedes it in the same sentence. A forbidden tool
call counts as executed unless it was rejected before dispatch (unavailable,
or its approval denied), because an error after dispatch may follow a partial
effect. A fenced JSON answer must be an object; anything else fails the
oracle's shape check. Code oracles run the visible and held-out suites in
separate fresh directories: the held-out tests are written only after the
visible suite has exited, a suite that runs no test fails, and so does one
whose test files changed while it ran. A legacy Agent has every Registry Skill
resident in its instructions, so its answer alone shows the Skill reached the
model; a `deferred@1` Agent must also have loaded the expected Skill, matched
by the Registry identity in the `capability_load` result. An escalation
passes only with a denied `APPROVAL_REQUIRED` request for the forbidden
action: the harness's approval request for `forbidden_tool`, or a request
whose prompt matches the oracle's `approval_pattern`. A live manifest keeps
that prompt only as `prompt_sha256`; a deterministic one keeps the fixture
prompt whole, so validation can regrade the oracle.

### Comparisons

A comparison changes only Agent definition fields; the task, its inputs, its
oracle and the evaluator stay identical, and each arm gets its own Agent
version and Workspaces. An arm may set only `projection_version`,
`prompt_cache` and `exposure`: the model, step limit, bindings and
instructions come from the task, so live authorization and spend reservations
price the Agent that actually runs. The corpus is refused otherwise. Live
Agents additionally remove every implicit builtin `remove_default` may name,
keeping only the Skill support tools their bound Skills need: delegation
(`task_create`, `task_delegate`, `agent_discover`) would start child Runs on
other Agents' models, and memory operations may call memory models, none of
which the ledger reserves or the evaluator cancels.

| Comparison     | Baseline → candidate                                | Issue |
| -------------- | --------------------------------------------------- | ----- |
| `projection`   | Legacy → `projection_version: ordered`              | #116  |
| `prompt-cache` | Ordered → Ordered with `prompt_cache: explicit`     | #142  |
| `exposure`     | Legacy exposure → `exposure: {version: deferred@1}` | #115  |

`completion-gate` (#112), `repository-rules` (#117) and `acp-adapter` (#121)
are reported as `feature-not-available`. `context-recovery` (#113, the Summary
Stage that distinguishes summary recovery from pruning), `parallel-read-tools`
(#119) and `streaming` (#120) are on this branch but have no comparison yet
(no task overflows a Context Policy, and the scripted provider does not
stream), so they are reported as `not-measured`: available, with nothing
measured. A comparison whose Agent
definition the Node rejects at registration is an error and fails the command;
only with `--allow-unavailable-arms` is it reported as `feature-not-available`
with the rejection as its `detail`.

### Adding a task

1. Put controlled inputs under `snapshots/<name>/` and held-out checks under
   `oracles/`.
2. Add a `[[tasks]]` entry with `id`, `kind`, `language`, `title`, `prompt`,
   `inputs`, `model`, `comparisons` and an `[tasks.oracle]` of one of the
   kinds in `ORACLE_KINDS` with the fields `ORACLE_FIELDS` lists for it
   (`scripts/fixed_task_evaluation.py`). Add a new oracle kind there, with a
   test, if none fits.
3. Name or add a scripted behavior in `Fixture.behave` and declare its
   expected deterministic result in `[tasks.expected]`.
4. Run `python3 scripts/test_fixed_task_evaluation.py` and the deterministic
   command. The corpus version should change when a task's meaning changes.

## Manifests

The result manifest (`tests/evaluation/schema/result.schema.json`) embeds the
experiment manifest (`experiment.schema.json`) and is validated against both
before it is written, together with cross-field rules: the `conclusion_scope`
is the evaluator's fixed statement; the recorded corpus and memory corpus
snapshots (version, combined digest and every file digest) equal those of the
corpora given to `validate`; every Run key names its
task, comparison, arm and repeat; every variant is a corpus pair with its
label and Agent fields, and every Run carries its pair's label, the task's
oracle kind and the digest of the task text; the recorded and skipped Runs
are exactly the planned keys (variants × repeats), with nothing dropped or
added, whether or not the manifest records an error; the calibration is
applied in the deterministic tier only and its failures are recomputed; every group
of Runs has exactly one aggregate whose outcome counts match its Runs, and
every aggregate has recorded Runs; every
aggregate's label, rates and metrics, every paired comparison and the memory
matrix summary equal what the recorded Runs and scenarios produce; each Run's
usage summary equals the one its Usage Records produce; each Run's outcome,
`long_context`, `tool_selection` and `constraint_retention` are recomputed as
`classify` chose them, from its recorded `infrastructure_error`,
`interrupted` flag, phase, oracle and error, and its oracle result agrees
with its checks; each deterministic Run's oracle is regraded from its recorded
final answer (which must match its digest), tool calls and human answers and
must reproduce the recorded oracle, apart from the test runner's output in a
code suite's `detail` (a code answer is executed only when it is one of the
fixture's own patches, so validation never runs code read from a manifest;
live evidence keeps only digests and cannot be regraded); the feature availability rows are exactly the corpus
comparisons, agree with it, and say `available` for every measured
comparison; the memory matrix reports every memory corpus scenario once, in
order, with its issue, evidence and coverage; a `passed` or `partial` memory
scenario has only passing checks and variants, and a `failed` one has a
failing check; no number is NaN or infinite,
although Python's `json` module accepts them; a live manifest has a spend
ledger and a deterministic one has none; and every spend attempt is charged
at its full reservation with no reported cost (the live tier reads no Usage
Record, so none is ever settled), the spend totals equal the sum of the
attempts' charges and their count, stay within the ceiling
the experiment declares, and include exactly one attempt per recorded Run.
A manifest that fails validation is still written, annotated in
its `error` field, because it may be the only record of paid work.

The experiment records the Aidash commit, dirty state and diff digest, the
binary SHA-256, the corpus and memory corpus snapshots, the evaluator identity,
the model registrations (config digests) and route, every arm's exact Agent,
model and tool registrations with the Agent fields that differ, the compaction
and Jev classifier modes, the Provider Credential and disclosure approval digest
(live), the repeat count, the time, token and spend limits, and the
reproducibility limitations. The live tier defaults to three repeats. A live experiment always sets
`model.mutable_alias: true`: an upstream alias can change without notice, and
this tier cannot read the provider-reported identity (see above), so a live
manifest cannot show what served a request.

Each `runs[]` entry has one `outcome`:

| Outcome                       | Meaning                                                                      |
| ----------------------------- | ---------------------------------------------------------------------------- |
| `success`                     | The Run completed and the oracle passed.                                     |
| `false_completion`            | The Run completed, but the oracle failed. The Agent's claim is irrelevant.   |
| `policy_compliant_escalation` | The Run completed by escalating as its escalation oracle requires.           |
| `unresolved`                  | The Run failed or was cancelled for any reason other than overflow.          |
| `overflow_failure`            | The Run failed because compaction could not fit, or the window was exceeded. |
| `interrupted`                 | The evaluator cancelled it at the time limit, or it never finished.          |
| `infrastructure_failure`      | The Node or the evaluator failed around the Run.                             |

Runs also record `long_context` (`none`, `pruning`, `summary_recovery`,
`terminal_overflow`), `constraint_retention`, `tool_selection`, wall time,
steps, compactions, per-request bytes and tool-definition counts (fixture
only), the Usage Record summary and records, the tool calls, the human answers
(live prompts as digests), and the evidence: input and answer digests,
verifier results, and for fixture traces the whole answer.

Usage follows ADR 0018: each token field is reported as `known` plus an
`unknown` count, never with unknown values as zero; a Run with any unknown
attempt has no total. Actual (`input_tokens`) and estimated
(`estimated_tokens`, the pinned Request Estimate) are separate, cache reads and
writes are breakdowns of input, and cost is split into known nanocredits and
uncertain attempts.

`aggregates[]` gives, per task, comparison and arm, the attempted count, every
outcome count and rate with its denominator (including
`claimed_success_without_oracle_pass`, `constraint_retention`,
`tool_selection_correct` and each long-context class), and
min/median/max/total distributions of the metrics. `comparisons[]` pairs the
arms, lists per-repeat outcome changes and the median deltas. Arms are
`comparable` only when both recorded exactly the same repeats; otherwise every
median delta is `null`.

## Memory matrix

`tests/evaluation/memory.toml` maps every row of #122's memory matrix to a
scenario with ground-truth sources, scopes, expected revisions, ordered
actions and expected corrections. Its `evidence` is one of:

- `deterministic-api`: the actions run through the authorized
  `memory/units/mutate` and `memory/units/query` endpoints with no model call,
  once per memory policy variant (`baseline-policy` and `decay-policy`), each
  in a new Workspace and participant bank. The manifest records each variant's
  bank and seed digest and checks that seeds are equal and banks distinct.
  These scenarios detect silent replacement from a clipped preview, lost
  concurrent writes, replay side effects, tombstone resurrection and reuse after
  deletion.
- `deterministic-test`: an existing deterministic test (the #140 paired decay
  fixture), run by the command.
- `feature-not-available`: #127 change semantics and #118 candidate review.
- `requires-live-tier`: retrieval quality for Japanese paraphrases, which a
  deterministic embedding cannot show.

The last two are reported with their status, never as passing. A
`deterministic-api` scenario marked `coverage = "partial"` observes only the
storage-safety half of its row, so its passing checks are reported as
`partial`: `exact-identifier-near-duplicates` shows that a correction changes
only the addressed unit but not ranked retrieval among near-duplicates, and
`empty-authorized-set` shows an empty authorized bank but not abstention under
recall. The matrix
reports storage safety separately from retrieval quality, extraction and answer
utility; this branch measures only the first deterministically.

## Limits

- One scripted probe per task; the deterministic tier says nothing about model
  quality.
- Synthetic token counts, prices and cache hits.
- Summary recovery (#113) and parallel read tools (#119) are on this branch
  but not compared yet; both are reported as `not-measured`.
- Live token counts come from Workspace events, capped at the latest 100; live
  spend is enforced from full per-Run reservations, not measured cost, and the
  provider-reported model identity is not recorded.
- The live tier cannot grade `python-tests` tasks until a container runner
  exists.
- The memory matrix's retrieval-quality rows need the live tier and an
  approved embedding model.
- The live tier cannot reserve or price Jev compaction calls; it relies on the
  evaluator's attestation that the evaluated Tenant approves no compaction Decider and
  stops dispatching when a Run shows an applied compaction.
- Neither tier measures pruning: the evaluated Agents bind no compaction
  Decider.
