---
status: accepted
---

# Bind transaction authority at each Participant admission

Each Participant must authorize the subject and exact local obligations in the immutable manifest when it durably admits the transaction. Later subject or transaction-trust revocation prevents new admissions but does not cancel an already admitted obligation; this permits recovery to finish a single durable decision without making committed work depend on renewed business permission. A coordinator submission is not admission by every Participant, and result reads continue to require current authorization.

This deliberately differs from ordinary execution, which rechecks authority at later work boundaries. If another Participant has not admitted the transaction when applicable authority is revoked, it must deny the new admission; already admitted Participants follow the coordinator's resulting decision. Admission is not a guarantee of COMMIT, cannot authorize a changed manifest, and grants no permission for unrelated work or data access.

Scoped submission requires dedicated transaction permission as well as every underlying mutation permission; it cannot turn the operator-only Registry registration operation into a subject operation. A transaction has one originating tenant and subject chain, with explicitly mapped local authority independently enforced at each participating Node.

Subject, mapping and transaction-trust revocation do not erase admitted obligations. Peer transport authentication remains mandatory: disabled peers or lost communication credentials may stall recovery until the same Node can be authenticated again, without restoring permission for new transactions. The initiating subject can inspect a transaction only with current read access to all its targets and can request abort only with dedicated authority while the decision is still open; operators retain recovery administration.

When a revocation races with admission and a remote response is lost, deny new work immediately but keep the revocation pending until the admission outcome is durably settled. Do not acknowledge completed revocation while stale preauthorization can still create a new admission; retain already admitted obligations for recovery. This accepts delayed revocation completion during a partition rather than falsely claiming that all uncertain admissions were prevented.

Accepted in the Issue #40 design interview on 2026-09-28, including the second- and third-round clarifications. The accepted constraints are in the [design record](../design/2026-09-28-transaction-acceptance.md); implementation and executed verification are tracked separately in the [evidence register](../operations/transaction-acceptance.md).
