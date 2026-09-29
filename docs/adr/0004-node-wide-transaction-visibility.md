---
status: accepted
---

# Retain Node-wide transaction visibility barriers

For v0.1.0 and Issue #40, retain the existing Node-wide visibility barrier while proving the cross-Node atomicity contract. A blocked transaction may therefore suspend ordinary reads, writes and Worker activity for unrelated Workspaces on a participating Node; limiting the barrier to individual resources would require a separate isolation and concurrency design. Expose the waiting reason and preserve the recovery control paths rather than releasing the barrier without a safe outcome.

Keep authority evaluation, revocation, authenticated restoration of the same peer, transaction status and undecided abort available through narrowly scoped control paths. These paths must not expose ordinary resource contents or bypass guards for unrelated application writes. Reject new subject requests promptly when a required reservation is busy, preserve exact-ID retries and admitted recovery, and do not block unrelated progress merely because an incomplete historical record remains.

Accepted in the Issue #40 design interview on 2026-09-28. This decision establishes the intended boundary, not passing release evidence; see the [design record](../design/2026-09-28-transaction-acceptance.md).
