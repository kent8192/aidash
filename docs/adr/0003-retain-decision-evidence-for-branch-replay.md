---
status: accepted
---

# Retain decision evidence for deterministic branch replay

Decision logs retain a state digest and access-controlled decision evidence by default; full Decision state retention requires explicit opt-in with separately agreed permissions and retention. Replay reconstructs the branch from recorded validated answers and exact effective rules, while a fresh provider inference is a diagnostic experiment because its probabilities need not match. This preserves branch auditability without making another full copy of private Agent history the default.
