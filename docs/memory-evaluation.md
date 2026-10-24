# Native memory evaluation

This is the measurement harness for Issue #122. It evaluates contracts (b) and (c)
of the [memory contracts](architecture.md#memory-contracts). Its results do not by
themselves establish release acceptance; see the [traceability matrix](memory-traceability.md).

`scripts/evaluate-native-memory.py` runs the frozen English/Japanese fixtures in
`tests/fixtures/native_memory_evaluation.toml` through the authorized native API.
Use an isolated Workspace with the selected exact Agent, memory policy and
PostgreSQL index configured, and a running Home maintenance worker. Each case
creates a fresh participant; the runner corrects only its own synthetic source.

```sh
python3 scripts/evaluate-native-memory.py \
  --base-url http://127.0.0.1:8080 --workspace WORKSPACE_UUID \
  --agent-id research --agent-version 1.0.0 \
  --provider-id native-memory --provider-version 1.0.0 \
  --server-revision DEPLOYED_SOURCE_FINGERPRINT \
  --output /tmp/native-memory-evaluation.json
```

Supply the authorized subject token using `AIDASH_EVALUATION_TOKEN` in the
process environment. It is never copied into the report. The output is created
exclusively with mode 0600 and contains raw synthetic claims and provenance.

The report records the fixture hash, declared tested revision/profile, extraction
label recall, useful retrieval recall, unlabeled claims, missing exact support,
duplicate labels, stale results after correction, exact retry equality and
durable charged calls/tokens/cost. Unknown provider outcomes remain conservatively
charged. API round-trip timings are reported separately for extraction, recall,
index readiness polling and correction; they are not database-only latency.
The authorized `usage` action is bounded to 128 operations per page and exposes
no claim text, saved authority or credentials.

Labels include bilingual names, preferences, negation and a failed verification;
the failed check must not be graded as verified success. Regex labels provide
reproducible contract measurements, not complete semantic judgments. Keep the
raw claims for human support/contradiction review. Exact deployment model,
embedding dimensions/version, reranker, tokenizer, price configuration and
Deployment Administrator approval must accompany a production evaluation.

Two-node native cluster acceptance invokes this same runner using its declared
synthetic model and zero-price policy. Those measurements prove the integrated
storage, extraction, retrieval, correction and accounting contract. They do not
approve production model quality, prices or Issue #73/#122 numerical targets.

The frozen fixture currently extracts all four expected labels in each locale
and retrieves three of four useful labels (recall 0.75). These synthetic results
do not approve production thresholds.

For actual completed-Run-to-human-review acceptance, run
`scripts/test-cluster.sh kubernetes native-memory learning-ui` with web
dependencies, a built frontend and Chromium. A local browser selects the actual
Workspace/bank and admits a real pending candidate through the live Home API,
in en-US/ja-JP for ordinary/generated Agents. Authentication presentation uses
the authorized bearer-session shim; memory, Registry and state requests are
real. This does not test OIDC login. Generated review/indexing retain the
original origin allowance/lifetime; retirement follows pending review/indexing
drain and current authority denies subsequent disclosure. HTTP-only `learning`
exercises the same canonical Run/evidence/accounting without a browser.

A Home Run's cumulative native read journal must fit its pinned provider's
`max_graph_visits`, including the Run root and the complete support graphs of
all delivered Unit revisions. Automatic learning also reserves its complete
`max_evidence` input/output envelope, which includes the Run root. Recall that
would exceed that allowance fails atomically before adding dependencies.
Local reads are staged across every declared bank and reflection step, then
committed together only after the complete tool result or combined inference
context succeeds and fits its delivery budget. The enclosing delivery transaction
owns that commit; failed commit, later-bank retrieval, or reflection leaves no
journal entries for the abandoned operation.
Retention excludes banks with active Workspace or settings readers before its
32-bank page limit, so busy banks cannot starve later maintenance candidates or
block an indexer's nested origin authority check.
Remote journals record only native Units retained in the final budget-fitted
receipt, in the same transaction as the completed attempt and receipt. Failed
finalization or commit leaves neither the receipt nor its native dependencies.
Purge invalidates and erases pending candidate quotations whose old Run support
exceeds a replacement policy's graph bound. A full candidate review queue leaves
learning pending for retry after review frees capacity; this wait preserves the
charged model receipt and does not consume a model failure attempt.

Usage-aware retention (#140) has a separate deterministic paired fixture,
`stale-relevant-versus-recent-noisy`, in the same TOML corpus. Its clock,
activation time, relevance arms, Unit identities and 24 periodic local Deliveries
are fixed. Both variants use identical authority, revisions, relevance and
candidate limits; only `decay` differs. A recent noisy keyword match must not
displace the older semantic and keyword support. The test uses the production
pure retention and RRF functions without credentials or live models:

```sh
cargo test -p aidash-domain memory::tests::fixed_clock_decay_evaluation
```

| #122 scenario                                   | Required observation                                                                  | Deterministic evidence                                              |
| ----------------------------------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| #140 stale relevant support versus recent noise | Matched decay on/off preserves relevant support; recency cannot manufacture relevance | Fixed-clock paired TOML fixture and retention/prior/tie regressions |

Diverse recall (#139) is opt-in through the pinned memory policy field
`diversity_lambda_millionths` (1..=1_000_000; 700000 is λ = 0.7). After reranking
and before token packing, recall selects `argmax λ·relevance − (1−λ)·max_sim(selected)`
over the already authorized candidates. Relevance is the reranker score divided by the
top score (shifted by the minimum when a score is negative); `max_sim` is the largest
non-negative cosine between the current embedding generation's stored vectors, so no
extra embedding call is made. Ties use the Unit ID. A derived Unit (Observation, mental
model) and the exact revisions it cites are structurally redundant: they count as maximally
similar (1.0) inside the same MMR step, so λ still decides whether a highly relevant source may
follow its summary, while a reasonably relevant unrelated Unit takes the slot first. Exact ties
use the baseline retention-then-Unit-ID order. A candidate without a current vector keeps
competing on relevance and is logged at debug level
(`memory diversification skipped candidates without a current embedding`); it is
never dropped. Diversification only reorders or drops authorized candidates. Without the
field the serialized policy and recall results are byte-identical to the baseline
(`tests/fixtures/native_memory_policy_baseline.json`); changing it is a new policy version.

The deterministic paired fixture `diversity_cases` in the same TOML corpus has an English
and a Japanese case: three paraphrases of one fact plus one distinct fact, and a delivery
budget that fits exactly two Units. Both variants share authority, candidates, relevance
scores and vectors; only the policy field differs. It runs the production recall path:

```sh
cargo test -p aidash-application near_duplicate -- --nocapture
```

| Case (λ = 0.7, budget for two Units) | Useful recall without → with | Delivered tokens without → with |
| ------------------------------------ | ---------------------------- | ------------------------------- |
| `english-near-duplicate-employer`    | 0.5 → 1.0                    | 1024 → 1009                     |
| `japanese-near-duplicate-employer`   | 0.5 → 1.0                    | 1055 → 1037                     |

Tokens are the UTF-8 bytes of the delivered envelope, which the pinned `utf8_upper_bound`
tokenizer charges. These synthetic numbers show the mechanism; they are not a measured
result for production models.

The live runner measures the same two quantities against real Homes. Register a second memory
policy that is identical except for `diversity_lambda_millionths`, an Agent bound to it, and pass
all four `--diversity-*` options. The runner then admits each fixture group (`diversity_cases`) as
separate Units on both Agents, sizes a delivery budget that fits exactly two of the live Units, recalls,
and adds `diversification.comparison` (per case and per language `useful_recall`, `delivered_units` and
`delivered_tokens` without and with diversification). Live embeddings and relevance replace the fixture's
fixed vectors and scores. The generic labelled cases also report `delivered_units` and `delivered_tokens`.

```sh
python3 scripts/evaluate-native-memory.py ... \
  --diversity-agent-id research-diverse --diversity-agent-version 1.0.0 \
  --diversity-provider-id native-memory-diverse --diversity-provider-version 1.0.0
```

`python3 -m unittest discover -s scripts -p test_evaluate_native_memory.py` covers the runner against
an in-memory Home. The live paired run needs a cluster and has not been executed for this change.

Memory policies may opt into integer `decay` parameters. Without that field,
serialized policies and recall ordering retain the baseline contract. Usage
counts distinct Runs receiving local completed Deliveries, follows Unit IDs
across revisions, and is committed with the read journal. Federated and remote
Deliveries do not increment local Usage. Enabling decay establishes a Bank
activation floor with no journal backfill.

Reactivation records a separate timestamp. Retention and dormancy measure age
from the latest of the Delivery (or learning), Bank activation and Reactivation
times, so correction, direct API Delivery, manual Reactivation and Unpin provide
a fresh interval without adding Usage.

Dormant and Pinned state do not change Unit content, visibility, revisions,
digests or dependent Runs. Default recall and reflection Read exclude current
Dormant flags. Evidence, consolidation, maintenance, correction, deletion and
purge retain their complete visible-unit boundary. Dormant Units count toward
record capacity. A policy-version mismatch or disabled dormancy makes stored
flags inert; maintenance reevaluates them with its persisted clock and cursor.

The existing authorized HTTP `POST /api/workspaces/{workspace}/memory/operate`
and its OpenAPI Operation schema expose these additional actions:

- `pin` and `unpin`, with `id`, require `memory.pin` and the participant or a Workspace administrator acting as a person.
- `reactivate`, with `id`, requires `memory.reactivate` and the participant or a Workspace administrator acting as a person.
- `dormant` lists Dormant Units under `memory.read_dormant`.
- `recall_including_dormant`, with `query`, requires `memory.read_dormant`
  in addition to ordinary read authority. Active and Dormant snapshots each
  retain their own fail-closed `max_units` bound.

Agent tokens cannot Pin or manually reactivate. `memory.read_dormant` requires
an explicit grant; native Agent tool contracts do not grant it by default.
Federation retains its existing Home-default recall protocol and exposes none
of these operations. Successful explicit local Delivery, correction, selection
as live derived support and Pinning reactivate a Unit. Keyword/entity hits do
not automatically reactivate it.

For these controls, Workspace ownership or the existing `workspace.update`
authority identifies administration; a bare memory-action grant to another
person does not grant administration of a participant Bank.
