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
and retrieves three of four useful labels (recall 0.75). The
[verification ledger](memory-verification.md) records
the exact profile and evidence. This is not production threshold approval.

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
