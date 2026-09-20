# Policy-driven agent generation

The generation service can complete a tenant-owned task for which no approved agent matches its requirements. A versioned policy embeds an Agent definition with an explicit model and approved tool/skill/cluster references. Requests cannot override the definition or permissions. The backend and bilingual dashboard implement this workflow. Approved compaction and the full failure/limit acceptance matrix remain outstanding before FR-AGENT-001 is complete.

## Policy and admission

Use `POST /api/generation/{tenant}/policies/{id}` with `expected_revision` (zero to create) and `spec`. The specification contains:

- `enabled`, `approval_required` and `template`, an ordinary Agent Registry entry.
- `permissions`: existing role/group IDs and generated subject attributes.
- `limits`: `max_agents`, `max_concurrent`, `max_depth`, `token_budget`, `tokens_per_agent` and `lifetime_seconds`.

All referenced components must already be approved in that tenant's catalog. The model must be selected explicitly; generation does not route between models. Policies and immutable revisions are stored alongside their actor and quota counters. A request pins its definition and permission specification to the policy revision; later changes affect new requests. Disabling the policy prevents further activation and generated execution at the next authority boundary. An unchanged policy can still be disabled after one of its components is revoked; re-enabling or changing its definition requires all current approvals again.

`POST /api/generation/{tenant}/tasks/{task_id}/assign` accepts `policy_id` and a nonempty `reason`. It first discovers a matching approved, executable ordinary agent. Otherwise it reserves a generated definition, concurrency slot and token allowance atomically. The worker's `task_assign` tool exposes the same operation with its inherited tenant identity. Operator credentials cannot originate a generation request; use a tenant subject credential.

The task's durable origin chain is inherited even when its root user submits the request through the API. Generated depth is derived from trusted ancestor records. The configured permissions and every originating subject must allow execution; a child cannot remove a parent's denial. Generated definitions are bound to their original task and cannot be executed through the legacy unscoped path.

The task ID is the generation idempotency key. An identical retry returns the existing record without consuming another slot. A changed policy, reason or origin conflicts. Reservations serialize concurrent requests, including requests using different policies for the same task.

## Approval, execution and history

A request begins in `PENDING_APPROVAL` or `QUEUED`. The control endpoint is `POST /api/generation/{tenant}/requests/{id}/control`, with `action` (`approve`, `deny`, `stop`, `delete`) and `reason`.

Approval queues the pinned definition. Provisioning atomically registers its immutable Registry version, tenant approval, delegated authorization subject, task claim, execution grant and run. Failure leaves no partial registration or orphan run. Every server and worker process runs a bounded reconciler; concurrent instances recheck state under the same database authority lock. Restarting resumes queued work without generating another ID.

A running request is `ACTIVE`. Completed, failed, denied, stopped and expired requests release unused reserved allowance once. The lifetime count is retained, including denied definitions; deleting a record does not reset quotas. Stopping or expiring an active request cancels its run, disables the generated authorization subject and catalog binding, and retains task journals and immutable definition/history. Deletion is a terminal-state tombstone, not destructive journal removal. In-flight effects finish under their authority lease; stop and policy disable wait for that boundary, and no subsequent model/tool boundary is authorized.

## Dashboard

Open **Agent generation** with a tenant subject credential to manage that tenant. An operator selects the tenant explicitly. Create or edit a policy using the model, tool and skill selectors, role/group IDs, permission attributes and numeric limits. The list shows each policy revision, lifetime definition count and allocated token allowance, with enable/disable controls.

On an open task, choose **Assign with policy** and record the reason. The generation page shows pending, running and terminal requests. Before approval, its detail view loads the immutable request-time policy snapshot, including permissions and limits; a later policy edit cannot change the displayed approval target. Approve, deny, stop and archive actions require a recorded reason. The same view shows charged tokens, inference attempts, origin chain, definition and lifecycle history. Archiving retains the record and task journals. Controls and responsive layouts support Japanese and English.

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

The budget is an input/output model-token allowance, not a monetary or arbitrary external-tool spending limit. Each generated definition reserves `tokens_per_agent` against its policy. Every model attempt first commits a conservative reservation of the approved context window plus maximum output, charging all generated ancestors as well as the generated executor. This prevents ordinary child agents from escaping an ancestor's token allowance.

Successful, complete, positive and bounded provider usage refunds unused reservation. Anthropic input usage includes its separate cache creation and cache read counters, following the [provider accounting contract](https://platform.claude.com/docs/en/build-with-claude/prompt-caching). Missing, invalid or uncertain usage remains charged in full. Crashes and failed attempts retain their reservation; a retry requires a new reservation. Request bytes must fit the reserved context window before provider I/O. These checks assume the approved provider honors its declared model limits; out-of-contract usage is an execution error and does not refund allowance.

Generated definitions currently approve only their explicit inference model. A context that requires the separately configured Jev provider fails before sending that history, because compaction has no generation-policy approval/budget contract yet. Short contexts require no compaction. Ordinary agents retain their existing Jev behavior. A policy-managed compaction allowance and the remaining process-failure/limit acceptance checks are required before this requirement can be declared complete.
