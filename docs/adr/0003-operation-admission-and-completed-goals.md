# Operation admission and conversation after goal completion

## Home-checked remote operation admission

Before admitting each new model or tool operation, a remote executor must obtain a current channel-eligibility decision from the channel's Home node. Do not reuse a time-limited eligibility approval across multiple operations. If the check cannot be completed, do not start that new operation. Operations without a monetary charge still require eligibility and operation authorization.

For billable operations, perform the current eligibility check at the Home-managed cost-reservation admission boundary. Eligibility, the current goal revision and execution scope, and the corresponding operation authority must permit admission; a budget reservation is not a substitute for authorization. Outcome recording and reconciliation for an already admitted operation do not authorize a subsequent operation.

Revocation cannot retroactively undo an external operation accepted before the revocation was committed. Preserve its outcome and cost settlement under the existing safe-boundary contract. Distinguish committed revocation from confirmed executor shutdown in the interface; an unreachable executor is not reported as confirmed stopped.

Per-operation checks favor current Home authority over independent remote progress during a Home outage. They fit the existing decision to reserve each billable operation at Home and avoid retaining a second, reusable window of channel eligibility.

## Conversation after goal completion

Keep a completed goal revision complete while allowing currently authorized continuing channel participants to answer questions about its existing results and shared history. A request to explain a conclusion or identify its recorded evidence does not by itself create a new goal revision or reopen completed tasks.

New investigation or external actions require a goal update through the normal confirmed, authorized path. Do not use a request for explanation to silently restart completed work. Each response still requires current participation, exact-version eligibility, read and operation authority, and any applicable channel-budget reservation. If the remaining allowance is insufficient, do not begin a billable response.

Do not revive a completed delegate's history or stream access to answer a follow-up question. Only an agent with independently valid current participation and authority may respond. Historical inspection remains read-only; switching to current conversation does not alter the historical result or authorize replay of past effects.

Separating completed work from discussion of its results preserves a usable conversation without treating goal completion as either a ban on explanation or a promise that future replies incur no cost.
