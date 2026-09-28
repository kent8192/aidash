# Independent Harness worker activation

Status: Q1-Q14 accepted on 2026-09-28. The user confirmed the shared understanding and authorized implementation with "実装開始". Runtime evidence is tracked separately.

This document records the design interview for [Issue #71](https://github.com/kent8192/aidash/issues/71). Open recommendations are proposals; accepted decisions are design agreements, not implemented behavior or passing acceptance evidence. Agreed domain terms are recorded in [CONTEXT.md](../../CONTEXT.md); architectural decisions with meaningful trade-offs are recorded in `docs/adr/` as they are settled.

## Verified starting point

Source inspection on 2026-09-28 used `origin/develop/0.1.0` at `20d1fb46f13cf042af6afce7763140c132428b78`. The original checkout is clean at `236bc28b099dcd77a6ae8df7608f84028d9b1fec`, the revision cited by the Issue. Design documents live on `docs/issue-71-worker-activation-design`; the original checkout was not updated.

- `src/main.rs` starts `EventBus::run` only when the mode is not `worker`. The newer development revision retains this condition.
- `src/bus.rs` publishes transactional events to a node-scoped JetStream subject. Its shared durable `execution` consumer commits a global inbox record, invokes process-local `notify_waiters()` only for a newly inserted record, then ACKs. It does not hand a notification to a separate worker process.
- `src/harness.rs::run_worker_until` scans immediately and after successful steps, then waits for local notification or a 500-ms timeout when idle. This is the normal activation path for a separate worker today.
- `src/store.rs::lease_run` selects eligible durable Run state under database locking and updates a fencing revision and lease. Eligibility includes control state, retry/wake times, human responses and core capability ordering. Preserving these conditions is part of this design.
- Existing Run processing retains transaction visibility gates, current authorization checks and unsafe-effect reconciliation. A notification is not evidence of permission or of whether an external effect happened.
- [Issue #70](https://github.com/kent8192/aidash/issues/70) is open and owns logical Agent subscriptions, recipient selection, durable recipient processing and Run/input linkage. [Issue #38](https://github.com/kent8192/aidash/issues/38) owns remote admission and Home-command authority.
- The [functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24) require event-driven execution/recovery and separate worker deployments. The [technology selection](https://app.notion.com/p/3e172fa877aa80e88c60d0dfeae01419) selects JetStream and separate Control API/Harness Worker roles. These requirements do not define the missing handoff contract.
- The current [nonfunctional requirements draft](../operations/nonfunctional-release.md) leaves performance and workload targets open under #73. A latency threshold selected here would be a component acceptance target, not an approved production SLO.
- [NATS pull-consumer documentation](https://docs.nats.io/learn/jetstream/pull-consumers) describes continuous pulls that receive new messages as they arrive. The accepted decisions below define Aidash-specific consumer topology, ownership and ACK boundaries.

No runtime or integration tests have been executed in this design interview.

## Existing constraints

Support combined and separate server/worker deployment. A server-only consumer must not consume the only activation that a worker needs. PostgreSQL remains the authority for accepted work, leases and effects. Broker failure must not gate HTTP/worker startup or lose accepted work. Preserve startup reconciliation and bounded recovery of missed notifications and expired leases. Require real PostgreSQL/JetStream evidence with separate OS processes and multiple workers; process-local notification is insufficient. Preserve logical recipient isolation, authorization, distributed visibility and unsafe-tool replay restrictions.

## Decision tree

The user accepted every recommendation in rounds 1-4. The behavioral decision frontier is empty. The [consolidated specification](2026-09-28-worker-activation-specification.md) records the resulting state machine, configuration and acceptance matrix; the user has confirmed this shared understanding.

| Decision | Agreed contract | Status |
| --- | --- | --- |
| Q1: Boundary with #70 | #70 owns recipient decisions and durable Run/input handoff. #71 owns cross-process activation of runnable Run state, including direct admission. Define a shared atomic handoff contract so committed runnable state cannot be separated from its durable obligation to notify. | Accepted |
| Q2: Worker eligibility and distribution scope | Use interchangeable Harness worker replicas within one executing Node and execution-capability profile for this Issue. Preserve all existing authorization and execution restrictions. Specialized capability/tenant pools and new placement policies require a separate design. | Accepted |
| Q3: Healthy-path acceptance | In a controlled fixture with one server OS process and two idle worker OS processes, complete startup reconciliation before admitting work. Delay periodic recovery scanning for 60 seconds during measurement. For 100 sequential admissions, require commit-to-first-valid-lease p95 <= 1 second and maximum <= 2 seconds, with notification-to-lease causal evidence and successful fixture execution. Use a scripted provider and real PostgreSQL/JetStream. This does not measure provider latency or production load capacity. | Accepted |

## Round 2: accepted decisions

| Decision | Agreed contract | Status |
| --- | --- | --- |
| Q4: Activation topology and ownership | Separate domain-event routing from Run activation with a dedicated node-scoped activation stream and one durable pull consumer shared by equivalent worker replicas. Only worker and combined modes pull activation notifications; server-only mode never does. Run the recoverable activation publisher in every mode so committed worker-side transitions do not depend on a live HTTP server. #70 retains ownership of its routing consumer and role placement. Combined mode follows the same durable activation contract. | Accepted |
| Q5: Activation coverage | Cover every existing transition that makes a Run eligible to advance: admission, accepted input when it permits progress, resume, human/approval responses, cancellation processing, step continuation, dependency release and transaction visibility release. Include persisted retry/wake/approval-expiry deadlines as normal scheduling work instead of relying solely on fallback recovery. Do not revive terminal Runs or bypass a current owner; #70/#38 and existing controls still determine legal transitions. | Accepted |
| Q6: Recovery cadence | Perform startup and reconnect reconciliation. Keep periodic recovery scans at intervals no longer than 5 seconds, shortening to no longer than 1 second while broker delivery is known unavailable. These cadences apply to eligible work with PostgreSQL available and worker capacity; they are not overall completion SLOs. A crashed worker's work becomes recoverable only after its current lease expires (currently 30 seconds), then enters the next recovery scan. Define jitter inside these bounds. | Accepted |

Q4 trades additional broker configuration and operational objects for explicit delivery ownership. [NATS worker-pool documentation](https://docs.nats.io/learn/jetstream/worker-pool) supports several processes sharing a pull consumer; the Aidash lease and effect contracts must still tolerate redelivery. The consolidated specification records stable identities, retention and provisioning.

Q5 follows source inspection of `Store::lease_run`, Run save/admission, human/approval controls and transaction completion. Time-based eligibility exists without a newly written event at the deadline, so notification-only code needs an explicit scheduling/reconciliation design. Q10-Q11 and the consolidated specification define scheduling and in-flight controls.

Q6 asks for the operational delay/cadence trade-off independently of the chosen transport. Faster fallback means more database activity. The implementation must keep recovery scanning effective while delivery failures are not yet classified. Q13 and the consolidated specification define reconnect timing, diagnostics and the distinction between scan cadence and end-to-end claim latency.

## Round 3: accepted decisions

| Decision | Agreed contract | Status |
| --- | --- | --- |
| Q7: ACK boundary | ACK only after the receiving worker commits a valid Run lease or a durable disposition proving this activation is already satisfied, no longer actionable, or still owed with an explicit future trigger/retry responsibility. Neither receipt nor local Notify is sufficient; do not hold the broker message through model/tool execution. A database failure or uncertain commit cannot produce a success ACK. Preserve pending responsibility when another worker owns the Run. | Accepted |
| Q8: Notification identity and content | Use a versioned reference envelope with executing Node, Run ID, activation ID and an activation generation distinct from the Run fencing revision. Keep event contents, prompts, credentials and effect instructions out of it. Reload authoritative state and conditionally claim the named Run under the existing eligibility/visibility rules. Deduplicate repeated publication by activation identity and database progress; do not discard newer input or rely on the broker duplicate window for correctness. | Accepted |
| Q9: Distribution and backpressure | Reserve a local execution slot before pulling a notification; allow at most one outstanding message per reserved slot and no speculative backlog beyond available slots. A busy replica must leave new work available to idle replicas. Keep Run-step ownership bounded to the existing durable step, preserve existing ordering constraints, and make continued runnable work eligible for redistribution. Do not promise exact round-robin balance or introduce tenant-priority scheduling here. | Accepted |
| Q10: Scheduled activation | Persist due times in PostgreSQL and have a bounded scheduler inspect only indexed, due activation obligations at intervals no longer than 250 ms. Competing instances in every runtime mode use expiring claims; due work enters the same durable notification path. Use database time and current eligibility/visibility checks. Keep this deadline scheduling separate from the 5-second/1-second broad recovery scans; startup/reconnect rebuild pending schedules from durable state. | Accepted |

### Edge cases the accepted contracts must address

- A worker commits a lease and dies before or after ACK: notification redelivery may occur, but no worker can bypass the valid lease or repeat an unsafe effect; lease-expiry recovery remains effective.
- A worker receives notification while another worker owns that Run: an active lease does not prove that the new input was observed. The new activation obligation must remain durable until an owner observes it or a later attempt can claim it.
- An older message arrives after newer input or a newer Run state: the handler reloads current authority and cannot regress state or mark a newer activation generation satisfied accidentally.
- A transaction commits but its visibility barrier remains closed: publishing, claiming and scheduled promotion must preserve that barrier. An early hint cannot authorize work, and pending responsibility survives until visibility release.
- One replica is doing slow inference while another has free slots: notification consumption must follow available capacity rather than buffering the backlog on the busy replica.
- A retry deadline arrives without a new domain event: deadline scheduling must produce the activation without requiring broad recovery scanning. Scheduling a future activation must not delay an already actionable one.

[NATS ACK/redelivery documentation](https://docs.nats.io/learn/jetstream/acknowledgment) defines explicit acknowledgments and delayed redelivery; Aidash's database disposition determines when an activation has been safely handed off. [NATS worker-pool documentation](https://docs.nats.io/learn/jetstream/worker-pool) describes demand-driven sharing of a consumer. Neither provides exactly-once effects without the application's lease and effect contracts.

Source inspection also confirmed that `wait_for_inference_cancellation` already checks the current Run's committed control every 250 ms. That owner-specific check is not a broad scan for runnable work. Q11 retains this mechanism with the existing input-ledger checks, while pending activation generations remain durable until a safe ownership boundary.

## Round 4: accepted decisions

| Decision | Agreed contract | Status |
| --- | --- | --- |
| Q11: Concurrent input and activation progress | Preserve existing step/input semantics rather than adding immediate model/tool preemption or owner-specific broker routing. Keep the committed-control check during inference and input-ledger checks before publishing a response or terminal completion. Track requested, claimed and settled activation generations separately from input observation. A worker can settle only the generation it claimed; newer generations remain pending. Atomically re-arm actionable pending work when a step releases its lease, and retain an expiry-based recovery obligation when another owner holds it. | Accepted |
| Q12: Broker storage and invalid notifications | Use WorkQueue retention with bounded storage (initial configurable limits: 24 hours and 1 GiB per executing Node), explicit ACK and DiscardNew when capacity is full. Keep accepted work and unresolved obligations in PostgreSQL regardless of broker retention or publication status. Quarantine malformed/unsupported/mismatched notifications using bounded, sanitized diagnostic metadata before terminating that delivery; never execute from untrusted contents or reinterpret unknown versions. Recover eligible compatible work from authoritative state. | Accepted |
| Q13: Reconnect and observable fallback | Retry broker operations in the background with exponential backoff from 500 ms to a 5-second maximum including jitter, with each connection/setup attempt bounded by 5 seconds. Re-establish the consumer and reconcile durable obligations before reporting restored delivery readiness. Keep startup and database-based readiness independent of broker availability. Expose event-driven/recovering/fallback/stopping state, sanitized reasons, due-work age, publication/ACK errors and recovery claims through operator diagnostics and private metrics. A TCP connection alone is not proof of restored delivery. | Accepted |
| Q14: Deployment lifecycle | Use additive migrations and a staged rollout: preserve database recovery during mixed versions, finish upgrading producers and workers, reconcile/backfill pre-existing obligations, then validate the notification path. Keep schema and durable obligations during rollback. On shutdown, stop new pulls/claims, settle only safely committed handoffs and drain current steps/HTTP for the existing 20-second budget. Leave unresolved deliveries/publications recoverable; do not delete the durable consumer or release a lease while its external effect might still run. | Accepted |

### Operational consequences carried into the specification

- **Q11:** Activation generations express scheduling responsibility, while `observed_input_seq` expresses input included in execution. Coalescing activation hints must never coalesce or delete the underlying input ledger or #70 recipient work. Completion/claim updates compare the captured generation and fencing token, never blindly copy the latest requested generation into a settled watermark. Due times retain every independent unresolved trigger; a future retry cannot postpone immediate eligible input, and visibility/control/ordering restrictions still apply.
- **Q11:** Distinguish an intentional new delivery from retrying an uncertain publish. Retries of one publication reuse its broker deduplication key; re-notifying a still-pending activation after deferral, ACK or broker loss advances a durable publication epoch. Otherwise the broker's duplicate window could suppress the scheduled notification. Logical activation identity and generation remain stable across transport retries, and database deduplication remains authoritative.
- **Q11:** Terminal Runs are not revived by stale activation. Existing durable delivery of accepted remote inputs/terminal messages remains the #38 contract; it cannot disappear just because Run leasing or activation becomes more selective.
- **Q12:** Broker storage is a bounded transport, not the audit authority. A publish ACK records transport persistence only; it cannot erase the database obligation or prove execution. Recovery can repair lost/expired broker records without redoing completed effects. Actual deletion of a settled activation record requires a retained Run generation/tombstone sufficient to reject historical duplicates; unresolved work is never deleted by a transport retention limit.
- **Q12:** On full storage, retain the failed publication in PostgreSQL and enter observable fallback; admission remains governed by existing admission limits. Successful fallback must not stop periodic broker repair. Worker scale-to-zero retains durable work; scale-up reconciles it. The 24-hour/1-GiB limits are configurable starting values, not supported production throughput or outage-capacity claims under #73.
- **Q12:** Invalid-envelope quarantine stores reason, bounded verified identifiers where available, payload digest and broker delivery metadata, not arbitrary payload bytes. If quarantine cannot be committed, do not terminate/ACK the delivery. An invalid envelope cannot advance any Run generation. Unsupported durable Run/schema semantics fail closed rather than being executed through fallback. Notification format compatibility is checked separately from Run compatibility.
- **Q12/Q14:** Use stable, collision-resistant identity derived from broker namespace, executing Node and protocol version, independent of Pod name. An operator provisioning command/manifest creates and validates production stream/consumer configuration; runtime credentials publish or pull only for their role. Local development may explicitly bootstrap those same objects. Mismatched broker configuration is surfaced as a configuration error with safe database recovery where Run compatibility permits; runtime must not silently delete/recreate an existing consumer. Stream replica count belongs to the deployment profile and is reported in evidence.
- **Q13:** Bound the entire connection/setup attempt, and keep recovery work independent of that attempt/backoff. Reset backoff only after successful setup and reconciliation. Metrics distinguish transport readiness from evidence of actual notification handling since reconnect; when due work exists, restored health requires progress, not just an open connection. The fallback cadence applies while reconnecting. PostgreSQL failure prevents unsafe claims and removes readiness under the existing contract.
- **Q13:** Aggregate metrics use bounded role/mode/reason labels; Node/Run/tenant/user identifiers and secrets are excluded from metric labels. Operator-authorized diagnostics may show permitted Run linkage and timestamps. Record the last successful recovery scan, oldest due obligation, broker-confirmed publication, worker disposition and lease acquisition, with both recovery and notification reasons distinguishable.
- **Q14:** Existing mixed-version writers can lack activation obligations; explicit reconciliation/backfill covers these writes until all compatible producers are deployed. A successful rollout requires a fresh notification-only-path test after old polling workers are drained. Rollback is conditional on schema/Run compatibility, without destructive down-migration. Keep leases fenced throughout drain/abort; an interrupted unsafe effect remains subject to reconciliation.

[NATS retention documentation](https://docs.nats.io/learn/jetstream/retention-policies) states that WorkQueue removes an acknowledged message and that stream limits still apply. PostgreSQL obligation repair is therefore required even with durable broker storage.

## Consolidation and confirmation

- The [specification](2026-09-28-worker-activation-specification.md) covers durable dispositions, generation races, scheduled promotion, invalid messages, broker loss, scale, shutdown and every Issue acceptance criterion.
- No new behavioral trade-off was found during consolidation. Reversible field/module/configuration names are concrete implementation guidance, not new product requirements.
- A final source drift check fetched `origin/develop/0.1.0` at `6b90aca`. Its four commits after the inspected `20d1fb4` do not change `src/main.rs`, `src/bus.rs`, `src/harness.rs`, `src/store.rs` or `src/transactions/gate.rs`. The design checkout stays at `20d1fb46f13cf042af6afce7763140c132428b78`.
- The user subsequently confirmed the design and authorized implementation. See the operations runbook and implementation evidence for verification; no PR publication is authorized.

## Decision record

- Q1-Q2: [ADR 0004: Persist the Run activation obligation with runnable state](../adr/0004-durable-run-activation-handoff.md).
- Q4: [ADR 0005: Give Harness workers a dedicated activation consumer](../adr/0005-worker-owned-activation-consumer.md).
- Q7-Q8 and Q10: [ADR 0006: Acknowledge durable activation dispositions before Run completion](../adr/0006-acknowledge-durable-activation-dispositions.md).
- Glossary: Harness worker, runnable Run, Run activation and activation generation are recorded in [CONTEXT.md](../../CONTEXT.md).
- Q3 is a component acceptance target retained here; it does not warrant a separate architectural decision record.
- Q5-Q6 and Q9 are accepted scope, operational and capacity rules retained here.
- User responses for rounds 1-4: all recommendations accepted. Implementation was separately authorized by the later "実装開始" request. No PR publication was requested.

- Q11-Q14 are accepted concurrency, transport and lifecycle policies retained here and expanded in the specification.
