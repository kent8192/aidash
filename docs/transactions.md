# Distributed transaction contract

The implementation follows FR-TX-001. Its release gate remains open until the complete process-failure matrix, subject authorization integration and expanded cluster acceptance are verified.

## Protocol and isolation

Each immutable manifest identifies a global UUID, coordinator, exact ordered participant set, serializable isolation, deadline and typed mutations. Registry versions remain immutable. Workspace, task and execution mutations specify expected revisions. Unknown mutation kinds, including ordinary external tools and A2A effects without a transaction participant, are rejected before admission.

Coordinators reserve participating nodes in ascending node-ID order. A node-wide visibility barrier is deliberately coarse: ordinary operations retain a shared database lease; reservation takes its exclusive counterpart and persists the transaction ID before releasing the database lock. New ordinary operations return a retryable unavailable response while this durable reservation exists. Short lock attempts do not queue behind a reserving transaction, preventing nested federation calls from creating a lock-order cycle.

Only after every node is reserved does preparation validate all local mutations inside a serializable SQL transaction and roll back its speculative changes. The manifest and prepared vote persist. The coordinator records an immutable commit or abort decision before sending finalization. Participants apply committed mutations and their events in one local transaction while retaining the visibility barrier. After every participant durably acknowledges application, the coordinator persists a visibility decision. Participants may then release their barriers. A delayed node returns unavailable rather than an older state; it already holds the committed data before any participant exposes new data.

Deadlines permit the coordinator to choose abort only while undecided. Participants never infer abort from a timeout or missing coordinator. A committed transaction continues recovery without expiry. Repeated messages must match the original manifest digest, and finalization reads the coordinator's durable decision. No compensation can replace an irrevocable commit. Recovery rotates through the least recently attempted incomplete transactions, so an unreachable aborted participant cannot indefinitely starve later work.

## Runtime boundaries

The visibility lease covers ordinary management and federation requests, Registry-backed discovery, worker steps including effects and journal persistence, generation reconciliation and outbox publication. SSE reacquires it for each read/delivery boundary. Database statement triggers additionally reject ordinary writes during a reservation, including writes to empty tables. Future application-table migrations must install the same guard. Transaction control/status, authenticated session identity and health remain available for recovery. Control queries have separate database capacity so ordinary callers cannot starve finalization.

Pending application requests return HTTP 503 with `Retry-After: 1` and `x-aidash-transaction-pending: 1`. Federation workers preserve their ordinary failure budget for this explicit condition; unrelated 503 responses retain normal failure/retry handling.

Administrative database access is outside the application protocol. Operators must not edit application rows or transaction records manually during an unresolved transaction. Transaction-aware runtime processes must be rolled out before accepting manifests; mixed old/new workers cannot preserve the barrier.

## Authority and acceptance

Transaction administration is initially operator-only. A configured federation peer needs a separate explicit transaction trust grant before reserving a node. Existing authenticated participants can finish their admitted transaction after that grant is disabled; disabling the grant prevents new reservations. Subject-scoped transaction admission remains part of the authorization integration gate.

Required evidence includes two independent databases; commit and abort; conflicting manifests; stale revisions; immutable decisions; each durable phase interrupted and restarted; partitions and duplicate/delayed messages; every ordinary read/write/worker/stream boundary; and no visible partial state or duplicate effects. Outstanding release acceptance is tracked in [the authoritative requirements in Notion](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24).

## Management workflow

The Japanese/English Transactions page lists coordinator decisions, participant acknowledgements, deadlines, wait reasons and durable history. Operators can review and submit an immutable manifest, abort an undecided transaction, and grant or revoke peer transaction trust. A committed transaction cannot be aborted. Session identity and transaction controls remain usable after reloading during a visibility reservation; ordinary cached data is hidden while its refresh is unavailable.

Configure and explicitly trust peers on both nodes before submitting. The coordinator must participate, participant IDs must be unique and sorted, and a new deadline must be within the next hour. Use the exact current revisions for workspace, task and execution changes. Submission returns HTTP 202 after durable admission; it does not mean commit or completion. Poll the transaction details until `complete` is true and inspect its decision.

| Mutation            | Preconditions and outcome                                                           |
| ------------------- | ----------------------------------------------------------------------------------- |
| `registry_register` | Validate and register an immutable Registry version                                 |
| `workspace_state`   | Replace object state at the expected workspace revision                             |
| `complete_task`     | Complete an owned running task with no unfinished children and publish one artifact |
| `finish_run`        | Finalize an idle execution at its expected revision, with no outstanding tool calls |

Delegated task completion must include the corresponding execution finalization on its recorded execution node; execution finalization must include completion on the recorded home node. Preparation invokes only the same SQL mutation code used at commit. Ordinary tool calls, provider requests and nonparticipating A2A side effects cannot be embedded in a manifest.

Protocol regressions are in `tests/transactions.rs` and `tests/transaction_protocol.rs`; the actual browser workflow is in `web/tests/transactions.spec.ts`. They distinguish fresh pool/HTTP-service restarts from an actual worker SIGKILL. These checks do not establish the remaining subject-scoped or Kubernetes/k3s integration gates.

## Verified acceptance

| Boundary                  | Evidence                                                                                                                                               |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Admission                 | Immutable/idempotent UUIDs, conflicting definitions, unknown external mutation rejection before work, and subject credential rejection                 |
| Two independent databases | Commit/abort, stale revisions, opposing coordinators, immutable decisions and exactly-once local updates/events                                        |
| Visibility                | Ordinary APIs, discovery, workers, SQL statement guards and an already connected SSE stream remain gated during partial application                    |
| Recovery                  | Fresh pools and HTTP servers at every durable transition; a real worker SIGKILL after COMMIT during a peer outage; duplicate and delayed messages      |
| Authority                 | Immediate abort on peer trust denial, coordinator impersonation rejection, and continued recovery after an admitted peer loses transaction trust       |
| Deadlines and fairness    | Undecided deadline abort and progress of later local work behind 33 unreachable aborted transactions                                                   |
| Dashboard                 | Review/submission, partition wait, reload during a reservation, abort, completed commit, stale-revision abort and trust revocation in Japanese/English |

The local full Rust suite passes 119 cases, including 12 two-node transaction protocol cases and the admission case. The two-node PostgreSQL/NATS dashboard acceptance passes all eight browser scenarios. Clippy with warnings denied, API/client generation, the production frontend build and Trunk pass. The complete all-phases OS-process/network failure matrix and the expanded authorization/cluster release gates remain open.

The local locking and isolation mechanics use PostgreSQL's [explicit locking](https://www.postgresql.org/docs/17/explicit-locking.html) and [serializable transactions](https://www.postgresql.org/docs/17/transaction-iso.html). The durable cross-node decision and visibility protocol is implemented by Aidash.
