# Distributed transaction contract

This document describes the implemented cross-node transaction protocol for supported manifest mutations. The [accepted design](design/2026-09-28-transaction-acceptance.md) defines FR-TX-001's boundaries; the [evidence register](operations/transaction-acceptance.md) distinguishes executed checks from the remaining Issue #40 release gates.

## Protocol and isolation

Each immutable manifest identifies a global UUID, coordinator, exact ordered participant set, serializable isolation, deadline and typed mutations. Registry versions remain immutable. Workspace, task and execution mutations specify expected revisions. Unknown mutation kinds, including ordinary external tools and A2A effects without a transaction participant, are rejected before admission.

Coordinators reserve participating nodes in ascending node-ID order. A node-wide visibility barrier is deliberately coarse: ordinary operations retain a shared database lease; reservation takes its exclusive counterpart and persists the transaction ID before releasing the database lock. New ordinary operations return a retryable unavailable response while this durable reservation exists. Short lock attempts do not queue behind a reserving transaction, preventing nested federation calls from creating a lock-order cycle.

Only after every node is reserved does preparation validate all local mutations inside a serializable SQL transaction and roll back its speculative changes. The manifest and prepared vote persist. The coordinator records an immutable commit or abort decision before sending finalization. Participants apply committed mutations and their events in one local transaction while retaining the visibility barrier. After every participant durably acknowledges application, the coordinator persists a visibility decision. Participants may then release their barriers. A delayed node returns unavailable rather than an older state; it already holds the committed data before any participant exposes new data.

Deadlines permit the coordinator to choose abort only while undecided. Participants never infer abort from a timeout or missing coordinator. A committed transaction continues recovery without expiry. Repeated messages must match the original manifest digest, and finalization reads the coordinator's durable decision. No compensation can replace an irrevocable commit. Active and aborted recovery use separate loops and connection capacity; unreachable aborted history cannot occupy the active loop. This guarantee assumes retained durable state. Permanent loss or inconsistent backup restoration requires the separate Issue #73 contract.

## Runtime boundaries

The visibility lease covers ordinary management and federation requests, Registry-backed discovery, worker steps including effects and journal persistence, generation reconciliation and outbox publication. SSE reacquires it for each read/delivery boundary. Database statement triggers additionally reject ordinary writes during a reservation, including writes to empty tables. Future application-table migrations must install the same guard. Transaction control/status, authenticated session identity and health remain available for recovery. Control queries have separate database capacity so ordinary callers cannot starve finalization.

Pending application requests return HTTP 503 with `Retry-After: 1` and `x-aidash-transaction-pending: 1`. Federation workers preserve their ordinary failure budget for this explicit condition; unrelated 503 responses retain normal failure/retry handling.

Administrative database access is outside the application protocol. Operators must not edit application rows or transaction records manually during an unresolved transaction. Transaction-aware runtime processes must be rolled out before accepting manifests. A mixed-version deployment requires evidence for the actual distinct, compatible runtime pair; same-binary retagging supplies no upgrade evidence. Removing the authority migration refuses pending coordinator or participant work; intake must be quiesced before downgrade.

Visibility is defined per application operation. Separate reads do not form a global multi-read snapshot. Durable events and mutations are applied once; delivery may repeat the same event identity and requires consumer deduplication.

## Authority and acceptance

Operators retain transaction administration and all supported mutations. Subjects may submit Workspace updates, Task completion and paired Run finalization. Registry registration, the local participant inventory, peer trust and peer authentication recovery remain operator-only.

Scoped submission requires `transaction.submit` plus the underlying read/mutation permissions. Task completion also requires `artifact.create` using the stored Task owner. Run finalization inherits the stored execution chain; intersecting permissions cannot be replaced by the caller's root subject alone. Source policies additionally authorize `transaction.submit` or `transaction.read` on foreign resources identified as `<node-id>:<resource-uuid>`. Each receiver requires an explicit source Node/tenant/subject mapping to a live local credential and independently evaluates its local policy.

The complete manifest is shared with every participant. Metadata-only preflight contains the transaction ID/digest, origin identifiers, recipient list and local target identifiers, without Workspace state or Artifact content. Every participant must authorize `transaction.disclose` for each local target and each `recipient_node` attribute before the coordinator persists or sends the full manifest. A denied recipient rejects the whole submission. Immutable preflight bindings prevent changing the identity, recipient set or target definition under the same ID.

Preflight and coordinator submission are not Participant admission. Each new reservation rechecks live source authority, the mapped receiver's credential/policy/chain and explicit peer transaction trust. The receiver commits its reservation while retaining the local authority locks. Source authority attempts are durable before peer I/O; the receiver retrieves a proof bound to the exact origin and manifest metadata. A lost reply remains an unknown outcome until a durable participant result or abort tombstone is reconciled.

Revocation denies new admissions. Credential/policy/trust changes return HTTP 202 with `pending_transactions` when an issued reservation still has an unknown outcome; this is not completed revocation. The operator can poll `/api/authorization/{tenant}/transaction-revocations`, or retry credential revocation, until those outcomes resolve. Admitted obligations survive subject, mapping and transaction-trust revocation. Transport still authenticates the peer: disabled/lost credentials can block recovery until restored for the same Node.

Policy inspection/evaluation, credential revocation, peer mapping changes and transaction recovery remain available during a barrier. Their database exception permits authority metadata and audit writes only. Issuing credentials, catalog changes and ordinary resource reads/writes remain gated. `POST /api/transactions/peer-recovery` restores an existing Node's credential reference and communication without changing its endpoint, mappings or transaction trust; enabled peers must still have distinct credential values.

Only the initiating subject with current access to every target can inspect the full transaction. Abort additionally requires `transaction.abort` and an undecided coordinator. New subject requests fail promptly with the explicit retryable 503 marker when a reservation is busy; exact-ID retries retain the existing transaction. If a remote busy reservation is found after submission, undecided scoped work aborts and reconciles all participants. An unrelated peer 503 never proves that admission was rejected.

Required evidence includes two independent databases; commit and abort; conflicting manifests; stale revisions; immutable decisions; each durable phase interrupted and restarted; partitions and duplicate/delayed messages; every ordinary read/write/worker/stream boundary; and no visible partial state or duplicate effects. The requirement is in [Notion](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24); #40 tracks the outstanding release evidence. The tests below cover a subset of this matrix.

## Management workflow

The Japanese/English Transactions page lists authorized coordinator decisions, participant acknowledgements, deadlines, wait reasons and durable history. Subjects can review, submit, inspect and abort within their current authority; operators also manage trust and inspect the Node-wide participant inventory. A committed transaction cannot be aborted. Session identity, transaction controls and the operator's authority page remain usable during a visibility reservation. Revoked transaction details and unavailable ordinary cached data are hidden.

An abort atomically competes with commit for the first durable decision. It can be recorded while recovery holds a transition lease for peer I/O; that temporary contention does not discard the operator's request. Repeated aborts retain one decision and audit record. A commit that wins the race returns a conflict to the abort caller. Participant finalization remains asynchronous and must still reach `complete`.

Configure and explicitly trust peers on both nodes before submitting. Scoped participants also need explicit mappings and the permissions above. The coordinator must participate, participant IDs must be unique and sorted, and a new deadline must be within the next hour. Use the exact current revisions for workspace, task and execution changes. Submission returns HTTP 202 after durable coordinator registration; it does not mean Participant admission, commit or completion. Poll the transaction details until `complete` is true and inspect its decision.

| Mutation            | Preconditions and outcome                                                           |
| ------------------- | ----------------------------------------------------------------------------------- |
| `registry_register` | Validate and register an immutable Registry version                                 |
| `workspace_state`   | Replace object state at the expected workspace revision                             |
| `complete_task`     | Complete an owned running task with no unfinished children and publish one artifact |
| `finish_run`        | Finalize an idle execution at its expected revision, with no outstanding tool calls |

Delegated task completion must include the corresponding execution finalization on its recorded execution node; execution finalization must include completion on the recorded home node. Preparation invokes only the same SQL mutation code used at commit. Ordinary tool calls, provider requests and nonparticipating A2A side effects cannot be embedded in a manifest.

Run `scripts/test-transactions.sh` for protocol, subject and real-process regressions. It selects this checkout's executable explicitly so another worktree's shared Cargo target cannot change the tested runtime. Run `scripts/test-cluster.sh kubernetes transactions` for disposable cluster acceptance. The separate `platform` profile retains the ordinary orchestration journey.

## Evidence inventory

| Boundary                  | Evidence                                                                                                                                                   |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Admission                 | Immutable/idempotent UUIDs, conflicting definitions, unknown external mutation rejection before work, and subject credential rejection                     |
| Two independent databases | Commit/abort, stale revisions, opposing coordinators, immutable decisions and exactly-once local updates/events                                            |
| Visibility                | Ordinary APIs, discovery, workers, SQL statement guards and an already connected SSE stream remain gated during partial application                        |
| Recovery                  | Fresh pools/HTTP services; real server SIGKILL before/after protocol persistence; real worker SIGKILL after COMMIT; duplicate/delayed messages             |
| Authority                 | Subject/mapped credentials, source/receiver revocation, mapping/trust revocation, unknown admission replies, disclosure denial and stored execution chains |
| Deadlines and fairness    | Undecided deadline abort and progress of later local work behind 33 unreachable aborted transactions                                                       |
| Dashboard                 | Review/submission, partition wait, reload during a reservation, abort, completed commit, stale-revision abort and trust revocation in Japanese/English     |

Tests are executable checks, not a release certification. The evidence register gives actual outcomes, image identities, distributions and exclusions, including unexecuted authority-race process cuts and distinct-version coexistence. Keep Issue #40 open until every required gate has its own evidence.

The local locking and isolation mechanics use PostgreSQL's [explicit locking](https://www.postgresql.org/docs/17/explicit-locking.html) and [serializable transactions](https://www.postgresql.org/docs/17/transaction-iso.html). The durable cross-node decision and visibility protocol is implemented by Aidash.
