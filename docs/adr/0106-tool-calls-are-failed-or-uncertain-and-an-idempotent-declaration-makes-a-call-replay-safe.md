# Tool calls are Failed or Uncertain, and an idempotent declaration makes a call replay-safe

Decided on 2026-10-30 for #594. A Tenant HTTP tool that answered 422, or whose service refused the connection, left its invocation unfinished. The worker retried the step, found the unfinished invocation and asked a person to reconcile an effect that never happened, telling them the worker had stopped. The model never saw the failure.

A call of a Tenant's HTTP or MCP tool, or of an Agent Plugin's MCP server, now ends in one of two ways when it does not succeed:

- **Failed Call.** The service answered with a failure, or the request certainly never reached it. The call completes with an error result that the model reads in its next step, and the Run goes on. Calls that were never sent, 408 and 429 answers, and 5xx answers of replay-safe tools are retried first: after 2, 4, 8, 16 and 32 seconds, like other outages, honouring a `Retry-After` of at most 32 seconds. A 5xx answer of any other tool fails at once, and its error says the change may have started. A refusal by the Deployment's outbound policy is a Failed Call, not an Authority Pause, because no change of the Run's authority can lift it. A 2xx answer whose body cannot be read completes as a success whose result could not be read, and is never sent again.
- **Uncertain Call.** The request may have reached the service, but the answer was lost, or the worker stopped. A replay-safe tool is called again under the same idempotency key. Any other tool waits for a person to reconcile it, and the request says which of the two happened.

A version that declares `replay = "idempotent"` makes its operation replay-safe. HTTP tools already send the idempotency key, and an idempotent MCP tool must name the argument that carries it. The declaration still needs Tool call approval: only a Read-only operation is exempt from approval (ADR 0078).

## Considered Options

- **Treat every non-2xx answer as uncertain.** That was the old behaviour. It hides recoverable model mistakes behind a wait for a person. Rejected.
- **Reconcile 5xx answers of Unsafe tools.** The service said it failed. The model, told the change may have started, can check or report it without a person. Rejected.
- **Keep `idempotent` as approval-only metadata.** Every lost answer would wait for a person even though the service deduplicates. Rejected.

## Consequences

- The fixed-task evaluator (#534) must record a reconciliation request as its own outcome instead of answering it as a Tool call approval.
