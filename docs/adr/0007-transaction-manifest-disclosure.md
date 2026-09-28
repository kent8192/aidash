---
status: accepted
---

# Require explicit authority to share the full transaction manifest

Retain a common immutable manifest shared with every participating Node, but require explicit authority to disclose its contents to every recipient before transmission or transaction admission. Transaction trust and resource mutation permission do not imply that one Participant may receive another Participant's Workspace values or Artifact contents. Reject a transaction if this disclosure cannot be authorized; a protocol with private per-Participant payloads and proofs is a separate design.

Recovery remains restricted to the accepted manifest and recipient set under the admitted obligations; it cannot introduce a new recipient or authorize ordinary data reads. Authorization preflight must not leak the protected payload in the request used to ask whether it may be disclosed.

Accepted in the Issue #40 design interview on 2026-09-28. See the [design record](../design/2026-09-28-transaction-acceptance.md) for authority boundaries and required negative evidence. This decision does not establish that disclosure enforcement exists in the current runtime.
