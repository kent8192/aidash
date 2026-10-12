# Semantic context when the embedding provider or vector backend is unavailable

Investigation for [#154](https://github.com/kent8192/aidash/issues/154), prepared against `develop/0.1.0` at `13a6f144`. The behavior below is pinned by regression tests added with this document; no runtime behavior changes. The decision itself belongs to the user, and the recommendation at the end is the one to accept or reject.

## Recommendation in one paragraph

Take **option 1 (degrade with an explicit marker), narrowed**: only optional workspace semantic context of a **local** Run may degrade, only after a bounded transient retry, only when the Registry-declared context source opts in, and only with a typed, journaled, visible marker. Everything else stays fail-closed: required-Home remote semantic context, every authorization, credential-binding and provider-contract failure, native memory banks, and every source that is not explicitly marked optional. **Independently of the option chosen**, a local Run must stop failing hard on an outage: today it ends `Failed` after one attempt while a required-Home Run retries and pauses with a typed reason. Options 2 and 3 are not recommended now (option 2 is impossible under #158, option 3 hinges on an undecided Provider Catalog question and costs an operational dependency); option 4 is the right stance for the contexts that must fail closed.

## 1. What happens today

Established by running the new tests against `develop/0.1.0`, not by reading code alone. "Step" is one `worker_once` of the Harness.

| Cause during a step                                    | Local Run (`auto_context` on)                                                                                                                                                                                                                                                                                  | Required-Home remote Run                                                                                                                                                                                                           |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Embedding provider answers HTTP 503                    | One attempt, no retry. The Run enters `Waiting(FailureDelivery { target: Failed })` with `error = "semantic backend unavailable or invalid; inspect index status and retry"`. The next step makes the Run and its Task `Failed`. No model request. `recovery.retry` and `recovery.semantic_reason` stay empty. | Transient `unavailable`: retry with a persisted delay of 2, 4, 8, 16 and 32 s; the sixth failure pauses the Run with `retries_exhausted`. Manual resume continues and keeps the attempt history. No model request. (existing test) |
| Embedding provider refuses connections                 | Same as 503, and the provider receives no query call.                                                                                                                                                                                                                                                          | Transient `unavailable`, `retry.count = 1` after the first failed step.                                                                                                                                                            |
| Embedding provider never answers                       | Same as 503 after the 20 s client timeout (3 s to connect).                                                                                                                                                                                                                                                    | Not run; every non-`RemoteSemantic` Home failure takes the same classification path as a refused connection.                                                                                                                       |
| Vector backend holds no points for the indexed sources | Same outcome, and the query is not even embedded: point presence is checked first.                                                                                                                                                                                                                             | Transient `unavailable`, `retry.count = 1`.                                                                                                                                                                                        |

Evidence:

- Local: `local_auto_context_step_fails_the_run_when_semantic_retrieval_is_unavailable` (four cases) in `server/src/apps/knowledge/tests/semantic.rs`.
- Required-Home: `semantic_retry_exhaustion_pauses_and_manual_resume_keeps_attempt_history` (existing, 503) and `required_home_semantic_outage_is_a_typed_transient_failure_for_every_cause` (new, refused connection and missing points) in `server/src/apps/identity/tests/scoped_remote_execution.rs`.

### Why the two paths differ

Every embedding failure is reduced to `SemanticUnavailable` (`aidash-application/src/semantic/embedding.rs`) and a missing vector point to the same error (`retrieval.rs`). The server's `classify_failure` (`server/src/apps/execution/repositories/worker.rs`) maps only the remote `RemoteSemantic(reason)` to `ExecutionFailure::Semantic`, which `aidash-harness/src/recovery.rs` retries and then pauses. A local `SemanticUnavailable` falls into the `_ => Other` arm, is not retryable, and is delivered as a terminal failure. The collapse also hides the cause: for an environment-keyed provider, `embedding::invoke` turns **every** provider error into `SemanticUnavailable`, so a rejected key (HTTP 401–403, remote `Configuration`) and a response-contract violation (`ProviderContract`) are indistinguishable from an outage on the local path. The difference is an accident of error mapping rather than a decided policy: the Issue's expectation ("an outage stops every step") understates it, because a local Run is not stopped but destroyed, and its Task with it.

### Scope of the exposure

- The failure applies to the Run's retrieval request before inference (`Executor::advance`, `ordered_semantic_context` / `semantic_context`). It does not occur when the index is disabled or `auto_context` is off.
- An Ordered Run reuses its retrieval while its Retrieval Key holds and rechecks authority without embedding (ADR 0020). From reading the code, such a Run survives an outage that starts mid-Run until its key changes (a new input, an index revision). A Legacy Run retrieves on every step. This reading is not covered by a test in this PR.
- Inference itself can be healthy: both paths stop before the model call, so the model request count is zero in every case above.

### Not covered by these tests

- The native memory bank path (`agent.memory`), which has its own recovery-serving gate and is exercised by `native_memory.rs`.
- A revoked or missing Provider Credential Binding. These are not outages: with a Tenant-backed embedding provider, `Invalid`, `NotFound`, `Forbidden` and `Conflict` are deliberately preserved instead of becoming `SemanticUnavailable` (`embedding.rs`), and a missing binding rejects admission before the Run exists (`docs/provider-credentials.md`).
- A PostgreSQL outage as the vector backend. Since ADR 0023 the vector backend is the application's own PostgreSQL pool, so it fails together with all durable state, which #156 row 4 accepts. The only independent vector failure left is missing or unreadable points, which the tests cover.

## 2. Failure classes

A policy should degrade only on the first class.

| Class                      | Examples                                                                                            | Today                                                                          | Should degrade?                                     |
| -------------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | --------------------------------------------------- |
| Outage                     | HTTP 408/429/5xx, refused connection, timeout, missing vector points                                | Local: terminal failure. Remote: retry, then typed pause.                      | Only for optional local context, after retry        |
| Authorization and binding  | Revoked or missing Provider Credential, Tenant mismatch, Forbidden, provider answers 401–403        | Admission rejection or `Authority` pause                                       | **No.** Silent continuation would hide a revocation |
| Provider contract          | Wrong model, wrong dimensions, oversized or malformed body (`provider_contract`)                    | Remote: typed pause. Local: terminal failure, indistinguishable from an outage | **No.** The provider is misbehaving, not absent     |
| Disclosure and consistency | `Invalidated` sources, `Authority`, `Configuration`, `ContextBudget`, generated-lineage `Allowance` | Typed pause                                                                    | **No**                                              |

## 3. Options against the criteria

Criteria from the Issue: correctness of Agent behavior without memory or workspace context, disclosure and authorization rules, cost of dual indexing, operational burden, consistency with #110 and #113.

|                                | 1. Degrade with marker                                                                                                                                                                                                                                                                                         | 2. Alternate provider                                                                                                                                                                                                                                                       | 3. Self-hosted embeddings                                                                                       | 4. Keep current (documented)                                                                     |
| ------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Correctness without context    | Step proceeds without retrieved text. Safe only if the model and the operator can see the absence; a bare `None` is indistinguishable from "no matches".                                                                                                                                                       | Full context kept, from a different embedding space.                                                                                                                                                                                                                        | Full context kept.                                                                                              | Run fails; no wrong answers, but no work either.                                                 |
| Disclosure and authorization   | Less is disclosed, never more. Must not apply to required-Home (receipts, dependency journals, ADR 0013 revalidation) or to authorization failures.                                                                                                                                                            | Another provider receives query text and, at indexing, source text: new disclosure target, needs its own approval and ZDR.                                                                                                                                                  | Disclosure stays inside the deployment.                                                                         | Unchanged.                                                                                       |
| Dual-indexing cost             | None.                                                                                                                                                                                                                                                                                                          | Two indexes: double embedding calls and tokens through the Embedding Allowance, double vectors (a default 3072-dimension `float4` point is about 12 KB), double sweeps, and a switch rule. An index is bound to model, version and dimensions, so vectors cannot be shared. | One index, but a model migration needs a rebuild.                                                               | None.                                                                                            |
| Operational burden             | Low: one policy field, one marker, one UI state.                                                                                                                                                                                                                                                               | High: two providers to approve, monitor and rotate.                                                                                                                                                                                                                         | High: GPU or CPU serving, capacity, upgrades, model licensing; a new single point of failure in the deployment. | None, but the OpenRouter embedding dependency remains a single point (#156 row 3).               |
| Consistency with #110 and #113 | Matches #110 ("optional new behavior is explicitly selected"; descriptors declare pin and disclosure class) if the choice lives on the source descriptor. Matches ADR 0010 and ADR 0023, which reject **silent** substitution and require typed, visible outcomes, only if the degrade is typed and journaled. | Contradicts ADR 0006's single OAuth credential path unless the second provider is also in the Provider Catalog.                                                                                                                                                             | Open Catalog question below.                                                                                    | Consistent with fail-closed; inconsistent with the typed-pause style of #113 for the local path. |
| Available under #158           | Yes.                                                                                                                                                                                                                                                                                                           | **No today.**                                                                                                                                                                                                                                                               | Undecided.                                                                                                      | Yes.                                                                                             |

### What #158 changes

- The Provider Catalog admits only providers whose OAuth flow issues a credential, so today's only entry is OpenRouter. A second embedding provider cannot be added, which makes option 2 **unavailable** until a second OAuth provider exists.
- Embeddings and inference then share one OpenRouter Provider Credential per Tenant. A revoked or missing binding stops both, so degrading embeddings buys nothing in that case, and by the table above must not be attempted. Option 1 helps in one scenario: **OpenRouter inference is healthy while its embedding route or model is down**, or the vector points are unavailable.
- Open Catalog question for option 3: whether a keyless in-deployment embedding model needs a Provider Catalog entry has not been decided. `EmbeddingConfig.validate_parameters` currently accepts only `openai` and `openrouter`. If the answer is yes, a keyless entry contradicts "each entry has a credential-issuing OAuth flow" and needs an ADR amendment.
- A finding for PR #219 (Phase 2 removes `credential_env` for embeddings): RequiredHome remote semantic search rejects any `provider_credential` embedding configuration (`aidash-application/src/authorization/source/semantic.rs`: "no local Run admission pin or approved BYOK maintenance authority"). With `credential_env` removed, a required-Home workspace embedding configuration has no valid form left. That is a larger problem than this outage policy, and it should be resolved before the required-Home half of any decision is relied on.

## 4. Recommendation

Choose option 1, narrowed as follows.

### What may degrade and what must fail closed

| Context                                                                                      | On outage                                                                                                                         |
| -------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Optional workspace semantic context of a local Run (`auto_context`, Registry source opts in) | Retry like a required-Home Run (5 attempts with backoff). If still unavailable, **degrade**: continue with a marker.              |
| Same context without the opt-in (default)                                                    | Typed transient pause (`retries_exhausted`, manual resume), the behavior a required-Home Run already has. Not a terminal failure. |
| Required-Home remote semantic context                                                        | Unchanged: typed retry then pause. Never degrades, because receipts and dependency journals are part of the disclosure contract.  |
| Native memory banks and any source declared required                                         | Fail closed with a typed pause.                                                                                                   |
| Authorization, binding, provider-contract, invalidation, budget and allowance failures       | Fail closed, as today.                                                                                                            |

### The marker

The memory bank already returns `{"status": "disabled" | "no_space"}` where it cannot supply content (`memory_context.rs`). Reuse that shape so the model sees the absence in the place the content would have been:

```json
"semantic_memory": { "status": "unavailable", "cause": "embedding_provider" }
```

`cause` is one of `embedding_provider` or `vector_backend`. It carries no endpoint, status body or query text. Rules the marker must follow:

1. **Not cached.** A degraded result must not be stored as the Source observation under the Retrieval Key (ADR 0020), or the Run would stay degraded after the provider recovers. The next step retries.
2. **Journaled.** Record the degraded step in the Run's events so the Task history and the API show that the step ran without semantic context, with the same typed cause.
3. **Visible.** The Run inspection API and the web UI show it. Degradation that only the model sees is a silent substitution.
4. **Opt-in.** The Registry-declared source descriptor carries `on_unavailable: pause | degrade` with `pause` as the default, frozen into the Run's binding snapshot like the Context Policy of ADR 0010.
5. **Bounded.** Optional, but a Run that degrades on every step for a long time should still surface it; a per-Run count in the inspection output is enough.

### Why not the others

- **Option 2**: impossible under #158 and, even if a second OAuth provider existed, doubles indexing cost and adds a disclosure target for a failure class the marker already absorbs.
- **Option 3**: moves the single point of failure into the deployment and depends on an undecided Catalog rule; revisit once the Catalog question is answered and only if degradation proves unacceptable for real workloads.
- **Option 4 alone**: leaves the inconsistent local/remote behavior and destroys a Task over a transient provider outage.

## 5. Follow-up Issues (not filed)

1. **`fix(execution): treat a local SemanticUnavailable as a typed transient semantic failure`**. Map `SemanticUnavailable` to `ExecutionFailure::Semantic(Unavailable)` so local Runs retry and pause like required-Home Runs. Stop `embedding::invoke` from collapsing key rejection (401–403) and `ProviderContract` into `SemanticUnavailable`, so only a real outage is transient and the other classes keep their typed pauses. Updates the four local regression tests added here, which currently pin the terminal failure.
2. **`feat(registry): declare on_unavailable (pause | degrade) on context sources`**. Descriptor field, validation, binding-snapshot freezing and migration of existing sources to `pause`; part of the #110 descriptor work.
3. **`feat(harness): degrade optional semantic context with a typed unavailable marker`**. Marker shape, no Source-observation caching, journaled event, interaction with the Retrieval Key and ordered reuse.
4. **`feat(api,web): show semantic context degradation on Runs`**. Run inspection field, event and UI state for a step that ran without semantic context.
5. **`docs: record the semantic context outage policy as an ADR and add the "Degraded context" term`**. Proposed wording is in the PR description.
6. **`fix(federation): define the embedding credential for required-Home semantic search after #158`**. RequiredHome search rejects Tenant-backed embeddings while #158 removes environment-backed ones; decide the supported path (relates to PR #219).
7. **`decision: does a keyless in-deployment embedding model need a Provider Catalog entry?`**. Gate for option 3; no implementation until answered.
8. **`test: Ordered Run keeps working through a mid-Run embedding outage while its Retrieval Key holds`**. Pins the reading in section 1 that is currently derived from code and ADR 0020.

## 6. Decisions needed from the user

- Accept option 1 as narrowed, or choose another option.
- Whether `degrade` stays opt-in per source (recommended) or becomes the default for optional workspace context.
- Whether the local path should retry five times before degrading (recommended, same as required-Home) or degrade at once.
