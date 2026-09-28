---
status: accepted
---

# Recover transactions from retained durable state with compatible runtimes

Issue #40 guarantees recovery from process failures and communication outages on the assumption that each Node retains its durable transaction state. When the decision cannot be established, participants wait rather than infer an abort; permanent data loss and inconsistent backup restoration require the separate Issue #73 restore contract. This accepts potentially prolonged unavailability instead of claiming progress without the durable authority for a decision.

Rolling-update acceptance covers same-version redeployment and explicitly verified compatible transaction-aware versions. Migration from a runtime that does not enforce the barrier requires disabling new transaction submissions and completing the runtime rollout before enabling them; mixing such a runtime with active transactions is unsupported. Compatibility must be demonstrated rather than inferred from a release number.

Compatibility evidence must identify an actually different old/new runtime pair and exercise server and Worker coexistence with transactions already reserved, prepared and committed. Relabeling the same binary does not qualify; if no eligible predecessor exists, the different-version upgrade gate remains unpassed.

Accepted in the Issue #40 design interview on 2026-09-28, including the second-round compatibility criterion. The compatibility execution and failure evidence remain open in the [design record](../design/2026-09-28-transaction-acceptance.md).
