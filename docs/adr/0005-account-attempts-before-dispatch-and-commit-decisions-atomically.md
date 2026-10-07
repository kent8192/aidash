---
status: accepted
---

# Account attempts before dispatch and commit decisions atomically

Every probability-provider request, including Shadow and retries, requires current authority and a durably charged attempt before network dispatch; generated chains also reserve each ancestor's allowance atomically within that owner's transaction. An applied Compaction branch, its evidence, and the Run context commit together under worker and revision fencing, while failures preserve the original context and uncertain calls remain charged. This gives up refunds for unprovable failures and availability when evidence cannot be committed in exchange for controlled disclosure, auditable decisions, and recovery that cannot silently apply an unjournaled branch.
