# Policy-driven agent generation

The generation service can complete a tenant-owned task for which no approved agent matches its requirements. A versioned policy embeds an Agent definition with an explicit model and approved tool/skill/cluster references. Requests cannot override the definition or permissions. The backend and bilingual dashboard implement this workflow. Its standalone generation acceptance gate is verified below. The expanded cluster, authorization, transaction, memory and A2A integration gates remain tracked in the implementation plan.

## Policy and admission

Use `POST /api/generation/{tenant}/policies/{id}` with `expected_revision` (zero to create) and `spec`. The specification contains:

- `enabled`, `approval_required` and `template`, an ordinary Agent Registry entry.
- `permissions`: existing role/group IDs and generated subject attributes.
- `limits`: `max_agents`, `max_concurrent`, `max_depth`, `token_budget`, `tokens_per_agent` and `lifetime_seconds`.
- Optional `compaction`: an approved `provider` reference, `calls_per_agent` and total `call_budget`. Omitting it forbids external compaction for that generated definition.
- Optional `embedding`: an approved embedding `provider` reference, `calls_per_agent` and total `call_budget`. Omitting it forbids embedding calls under that generated authority.

All referenced components must already be approved in that tenant's catalog. The model must be selected explicitly; generation does not route between models. Policies and immutable revisions are stored alongside their actor and quota counters. A request pins its definition and permission specification to the policy revision; later changes affect new requests. Disabling the policy prevents further activation and generated execution at the next authority boundary. An unchanged policy can still be disabled after one of its components is revoked; re-enabling or changing its definition requires all current approvals again.

`POST /api/generation/{tenant}/tasks/{task_id}/assign` accepts `policy_id` and a nonempty `reason`. It first discovers a matching approved, executable ordinary agent. Otherwise it reserves a generated definition, concurrency slot and token allowance atomically. The worker's `task_assign` tool exposes the same operation with its inherited tenant identity. Operator credentials cannot originate a generation request; use a tenant subject credential.

The task's durable origin chain is inherited even when its root user submits the request through the API. Generated depth is derived from trusted ancestor records. The configured permissions and every originating subject must allow execution; a child cannot remove a parent's denial. Generated definitions are bound to their original task and cannot be executed through the legacy unscoped path.

The task ID is the generation idempotency key. An identical retry returns the existing record without consuming another slot. A changed policy, reason or origin conflicts. Reservations serialize concurrent requests, including requests using different policies for the same task.

## Approval, execution and history

A request begins in `PENDING_APPROVAL` or `QUEUED`. The control endpoint is `POST /api/generation/{tenant}/requests/{id}/control`, with `action` (`approve`, `deny`, `stop`, `delete`) and `reason`.

Approval queues the pinned definition. Provisioning atomically registers its immutable Registry version, tenant approval, delegated authorization subject, task claim, execution grant and run. Failure leaves no partial registration or orphan run. Every server and worker process runs a bounded reconciler; concurrent instances recheck state under the same database authority lock. Restarting resumes queued work without generating another ID.

A running request is `ACTIVE`. Completed, failed, denied, stopped and expired requests release unused reserved allowance once. The lifetime count is retained, including denied definitions; deleting a record does not reset quotas. Stopping or expiring an active request cancels its run, disables the generated authorization subject and catalog binding, and retains task journals and immutable definition/history. Deletion is a terminal-state tombstone, not destructive journal removal. In-flight effects finish under their authority lease; stop and policy disable wait for that boundary, and no subsequent model/tool boundary is authorized.

## Dashboard

Open **Agent generation** with a tenant subject credential to manage that tenant. An operator selects the tenant explicitly. Create or edit a policy using the model, tool and skill selectors, role/group IDs, permission attributes and numeric limits. The list shows each policy revision, lifetime definition count and allocated token/call allowances, with enable/disable controls. Compaction and embedding selectors use tenant-approved providers and expose per-agent and total call limits.

On an open task, choose **Assign with policy** and record the reason. The generation page shows pending, running and terminal requests. Before approval, its detail view loads the immutable request-time policy snapshot, including permissions and limits; a later policy edit cannot change the displayed approval target. Approve, deny, stop and archive actions require a recorded reason. The same view shows charged tokens, inference attempts, compaction and embedding calls/limits, origin chain, definition and lifecycle history. Archiving retains the record and task journals. Controls and responsive layouts support Japanese and English.

## Read APIs

Read policy summaries at `GET /api/generation/{tenant}/policies`, requests at `GET /api/generation/{tenant}/requests`, and lifecycle history at `GET /api/generation/{tenant}/requests/{id}/history`. Read the request-time policy specification at `GET /api/generation/{tenant}/requests/{id}/spec` and token accounting at `GET /api/generation/{tenant}/requests/{id}/usage`. The usage query observes both counters and committed attempts in one database snapshot. Request collections, pinned specifications, usage and workspace events require generation read permission as well as workspace access. Credentials are never included in the response.

| Action               | Purpose                                                |
| -------------------- | ------------------------------------------------------ |
| `generation.manage`  | Create or revise a policy                              |
| `generation.request` | Request assignment or generation                       |
| `generation.read`    | Read policies, requests, history and generation events |
| `generation.approve` | Approve or deny a pending definition                   |
| `generation.stop`    | Stop pending, queued or active generation              |
| `generation.delete`  | Tombstone a terminal request                           |

Ordinary workspace, task, Registry, model, tool and execution permissions are also required. Approval cannot supply a missing originating permission. Activation rechecks the original credential and current policy/catalog approvals. Revocation before activation records a failure; revocation during execution pauses it at the next boundary.

## Budget contract

The budget covers inference input/output and embedding input tokens, not monetary or arbitrary external-tool spending. Each generated definition reserves `tokens_per_agent` against its policy. Every model attempt first commits a conservative reservation of the approved context window plus the model's catalog maximum output, charging all generated ancestors as well as the generated executor. This prevents ordinary child agents from escaping an ancestor's token allowance.

Successful, complete, positive and bounded provider usage refunds unused reservation. Anthropic input usage includes its separate cache creation and cache read counters, following the [provider accounting contract](https://platform.claude.com/docs/en/build-with-claude/prompt-caching). Missing, invalid or uncertain usage remains charged in full. Crashes and failed attempts retain their reservation; a retry requires a new reservation. Request bytes must fit the reserved context window before provider I/O. These checks assume the approved provider honors its declared model limits; out-of-contract usage is an execution error and does not refund allowance.

## Approved compaction

Register a `compactor` Registry entry and approve its exact version in the tenant catalog. Its configuration selects `provider: "typesafe-system-one"`, `endpoint`, `model`, `credential_env`, `max_request_bytes` (1024–1048576), `max_questions` (1–1024) and `max_response_bytes` (128–1048576). Credentials use configured `AIDASH_SECRET_*` references. The bilingual Registry form exposes these fields. HTTPS/HTTP endpoints must not contain credentials, query strings or fragments; approved calls never follow redirects, have a 5-second connection timeout and a 30-second request timeout, and enforce response size while reading.

A policy explicitly references that entry:

```json
{
  "compaction": {
    "provider": { "id": "approved-jev", "version": "1.0.0" },
    "calls_per_agent": 10,
    "call_budget": 100
  }
}
```

The request pins this contract to its immutable policy revision. Each generation reserves its per-agent call allowance against the policy total. Termination returns unused allowance exactly once; consumed calls remain allocated. Before each System One HTTP attempt, the worker checks live authority, `registry.read`, `compaction.invoke`, catalog approval, expiry and the provider agreement of every generated ancestor. An ordinary child executing within a generated chain inherits this contract too. All generated ancestors must approve the same exact provider version; an omitted or different reference rejects external compaction before disclosure.

Input bytes and question counts are checked before reservation. The worker then atomically commits one call and an attempt record for every generated ancestor before network I/O. The ledger retains provider/version, run, byte count and question count, without storing the request history or credential. Concurrent batches cannot overspend a call limit. Failed, uncertain and crashed attempts stay charged, and retries reserve new attempts. System One probability responses do not report trustworthy model-token usage, so this is a separate call budget, not a fabricated token refund or monetary limit. Main inference reserves its tokens after successful compaction and rechecks lifetime before its own HTTP request.

No configured compaction permission is needed when the context already fits. Generated chains never fall back to the node's environment-selected Jev provider. Ordinary agents outside a generated chain retain that provider selection, with bounded 1 MiB request/response payloads and at most 1024 questions per request.

## Approved embeddings

Register an `embedding` Registry entry and approve its exact version in the tenant catalog. The bilingual Registry form exposes the OpenAI-compatible API base endpoint, model, model version, vector dimensions and optional `AIDASH_SECRET_*` credential reference. This configuration must equal the workspace index's `embedding` configuration, including its endpoint, model/version, dimensions and credential reference. Matching just the display name or model is insufficient.

Select that provider in the generation policy and set per-agent and total call allowances:

```json
{
  "embedding": {
    "provider": { "id": "approved-embedding", "version": "1.0.0" },
    "calls_per_agent": 10,
    "call_budget": 100
  }
}
```

The request pins the provider and limits to its policy revision. Before a query or background indexing call, every generated ancestor must remain active, unexpired and enabled, and approve the same provider version. Current catalog approval, `registry.read` and `embedding.invoke` must allow all originating subjects. Background memory indexing restores the original credential and subject chain instead of borrowing operator authority. Changing a workspace index cannot expand that pinned approval.

One call and an input-token reservation of UTF-8 input bytes plus 1,024 framing tokens commit for every generated ancestor before HTTP. The call allowance is independent, while embedding tokens share the inference token budget. Valid, positive, equal `prompt_tokens` and `total_tokens` refund only unused reserved tokens. Missing or malformed usage keeps the full reservation. Usage above the reserved bound fails execution without a refund. Failed or interrupted calls remain charged; retries require new allowance. Concurrent indexers cannot fund the same attempt twice. Terminal generation releases unused allocation once while retaining consumed calls and tokens.

`generation_embedding_usage` retains the attempt ID, request, workspace, optional run/source, provider version, input byte count, reservation and reported usage. It stores neither source text nor credentials. The dashboard displays pinned approval and charged embedding calls in each request and total allocated calls in its policy. Ordinary callers outside a generated chain keep the operator-configured workspace index behavior.

## Acceptance evidence

The fixtures use local HTTP providers and real PostgreSQL; no paid provider quality is inferred.

| Contract                            | Verification                                                                                                                                                             |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Missing matching agent              | Generated definition completes its task; matching ordinary agents are reused without reserving generation quota                                                          |
| Retry, approval and provenance      | Identical assignments/controls retain IDs, pinned revisions, origin and history; denial prevents activation                                                              |
| Count, concurrency and total tokens | Each limit independently rejects a new request without partial state; raising only that limit admits the same task                                                       |
| Generated depth and authority       | Worker-created task origins retain parent restrictions even when the root submits assignment                                                                             |
| Per-agent inference tokens          | Unknown/incomplete usage keeps the full reservation and blocks another over-budget model call                                                                            |
| Tool permission denial              | A generated `/team` attribute triggers ABAC denial before HTTP; the pending tool cursor survives and runs once after policy correction and explicit resume               |
| Lifetime, stop and revocation       | Expiry and revoked credentials prevent provider calls; stop waits for an in-flight lease and blocks the next boundary                                                    |
| Compaction                          | Long history is pruned with the pinned provider; call reservations commit before HTTP and intersect all generated ancestors                                              |
| Compaction limits and failures      | Concurrent allocation is bounded; failed calls stay charged, exhausted budgets prevent HTTP and unused allowance releases once                                           |
| Generated semantic memory           | Queries and background indexing require the pinned provider, intersect ancestor authority and charge shared tokens plus embedding call limits                            |
| Embedding recovery                  | Concurrent indexers, failed/unknown/overreported usage, expiry, catalog denial and real worker SIGKILL/restart retain durable reservations and prevent unfunded calls    |
| Process failure                     | Real worker processes are killed during inference and compaction; new processes finish the original run with one generated definition and retained uncertain usage       |
| Tenant and read scope               | Requests, specifications, usage, histories and events honor tenant/read permission boundaries                                                                            |
| Management dashboard                | Japanese/English policy creation/editing, compaction selection, immutable approval review, completion, denial, stop, archive and enable/disable pass in the real browser |

The cases are in `tests/generation.rs`, `tests/generation_compaction.rs`, `tests/generation_semantic.rs`, `src/context/jev.rs` and `web/tests/generation.spec.ts`. The broader two-node dashboard golden path also passes. These results establish the standalone generation and local semantic integration contracts; the six-capability integrated release gate remains separate.
