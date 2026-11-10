# Decision gate

The `DecisionGate` evaluates version-pinned approval and context-selection Deciders. Context recovery does not invoke a Decider (ADR 0125). Decision Attempts and Evidence remain available to the approval and observation paths.

## Approval hook

The approval Decider judges one pending Tool call of a Run for the Auto Approval Mode (ADR 0077). It is a Decider of hook `approval`. It uses the DecisionGate and never pauses a Run for its own sake. The Tenant chooses its version. Until the Tenant names another `decider` of hook `approval`, the version is the built-in `aidash.approval-decider@1.0.0`, which Aidash seeds into every Tenant Registry. Auto needs a Catalog Approval of the exact version on the Node. The routes that read and set the choice are in [Approval Rules and Modes](authorization.md#approval-rules-and-modes).

The built-in version runs in `enforce` mode with `max_calls_per_run` of 200. It tries two Routes in order. `typesafe/jev-1.14` runs through System One. `cloudflare/clef` runs through OpenRouter's Decisions API on `cloudflare`, `deepinfra` and `primeintellect`. Both Routes block at `threshold` 0.3, which is not yet calibrated for approvals. A changed threshold is a new version. Approving the version approves sending a Run's state to each Route.

The Decider asks five questions about the call, one for each block reason: `data_exfiltration`, `destructive`, `beyond_request`, `injected` and `boundary`. A category blocks the call when its probability reaches the effective `threshold`. The most probable blocking category names the reason, and on a tie the category listed first wins. A call that no category reaches is allowed. A lower `threshold` is stricter. An Agent's Binding can lower a Route's threshold, but it never raises one.

The state holds the inputs people sent to the Run's Thread, the Agent's instructions, the Run's earlier tool calls without their results, and the pending call with its tool's definition. It never holds tool results or the model's own text. Only inputs whose author is an enabled user subject of the Run's Tenant count as people's inputs, so a message another Node's agent sent never does. Text a tool returned reaches the Decider only where the model copied it into a call's arguments.

An allowed call runs at once, without another model response. A blocked call is refused, and the model receives `tool call was blocked by Auto: <reason>` as its error. The reason is one of the five block reasons, or `unavailable` when no Route answered. An unavailable approval Decider never pauses the Run. A version in `shadow` mode records its verdict and settles nothing, so the person is asked as in Manual. A spent `max_calls_per_run` allowance records no decision and leaves the call to a person.

Each decision records an `ApprovalEvidence`. It holds the `verdict`, the `answers`, the Route that answered (`route`), the Routes that were unavailable before it (`failed_routes`), the effective `restrictions` and the `binding_restrictions`, the `mode`, and the `attempts`. It keeps the digest of the state, not the state. A verdict from the Route asked can be checked again from its answers and the effective threshold.

## Persistence

Decision Attempts are reserved under the Run lease before dispatch. An uncertain attempt is not dispatched again. Approval Evidence records the verdict and durable attempts without retaining the classified state. The Node rechecks the exact Decider version and its Catalog Approval at the decision boundary.
