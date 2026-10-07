# Native memory evaluation

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
operator approval must accompany a production evaluation.

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
