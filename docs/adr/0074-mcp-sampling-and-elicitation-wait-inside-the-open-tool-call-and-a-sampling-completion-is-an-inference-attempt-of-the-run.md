# MCP sampling and elicitation belong to one tool operation, and sampling is an Inference Attempt of the Run

Decided on 2026-10-19 and amended on 2026-10-23 while grilling Agent Plugins 1.0.0. Sampling and elicitation belong to one logical MCP tool operation until it finishes or fails. On the legacy protocol, they arrive while a wire call is open: the Run keeps its lease and session, and expiry returns a refusal or decline to the server so that the outer call can continue. On MCP 2026-07-28, a received `InputRequiredResult` is an acknowledged continuation checkpoint: Aidash saves it and releases the worker lease during human waiting. A stdio Pod remains frozen with its Working Area ownership retained. Resumption continues the original operation under its original Tool call approval.

A Sampling approval is addressed to a Designated approver and may be given once or as a Run grant for that server. Approval Modes apply: Don't ask refuses, Bypass approves within the Tenant's policy, and Auto asks a person. The completion is written by the Run's own model through its Provider Credential Binding, as an Inference Attempt of the Run, without any of the Run's context, without tools and with a capped `maxTokens`. An Elicitation is answered by the people of the Run's Thread, like the model's question. Both respect the cumulative MCP human-wait allowance and approval expiry, and Elicitation also respects Answer Deadline.

## Considered Options

- **Release the lease and replay an unacknowledged legacy call.** The server's session would be gone, and an `Unsafe` call could run twice. Rejected. This differs from continuing a modern call from an acknowledged input-required checkpoint.
- **Refuse sampling altogether.** Rejected by the maintainer; sampling is supported with its own approval.
- **Let the Agent version's Catalog Approval admit sampling up to a budget.** Nobody would decide individual requests. Rejected; Run grants and Approval Rules keep the load low instead.
- **Keep sampling cost out of Usage Records,** as Decision Attempts do. The cost would escape the Run's budget. Rejected.
- **Approve the request and then the completion.** The completion carries none of the Run's context, and two waits rarely fit within the expiry. Rejected.

## Consequences

- Legacy waits hold a worker lease and session. Modern human waits use a durable checkpoint; both need approval notifications within their waiting limits.
- Auto never approves sampling by itself until a Decider can answer a question about it.
- Workbench tests deny sampling and elicitation, as they deny other approvals.

## Amendment accepted on 2026-10-23

The Deployment sets a separate ceiling for waiting for a person's answer to an MCP request, defaulting to 15 minutes. A Sampling approval waits for at most the shorter of that allowance and the approval expiry. An Elicitation also respects the person's Answer Deadline. This replaces using the approval expiry alone for these waits: approval autonomy made its default 24 hours, which would otherwise retain a legacy worker lease and potentially an Execution Pod throughout that interval.

A Run actively processes at most one sampling or elicitation request at a time. Overlapping legacy requests receive an error instead of being queued. Modern input-required responses may contain a bounded batch: Aidash displays and executes its inputs serially, and sends one continuation only after all required results are ready. This keeps human interactions unambiguous and avoids concurrent Sampling Inference Attempts for the same Run.

The MCP human-wait allowance is cumulative within one outer tool call. Each subsequent request receives only the remaining allowance, also bounded by its approval expiry and any applicable Answer Deadline. A server cannot extend the wait indefinitely by issuing requests in sequence.

On the legacy protocol, failure to reserve the Run's token budget or failure of its provider call returns an error to the sampling request while the usable session's outer tool call continues. On the modern protocol, sampling refusal, expiry, budget failure or provider failure finishes the logical tool call with a typed tool error and sends no continuation: its input-response type has no error variant. Elicitation decline and cancel are valid results and may continue normally. A batch is never continued with missing results or a fabricated completion.

Existing usage reservation, refund and unknown-usage accounting apply in both protocols, including work already performed before a later batch input fails. A server sampling request gets one Inference Attempt with no automatic retry. A durable resume reuses a journaled accepted response; uncertainty is accounted for as uncertainty rather than starting another provider call. Later server requests undergo approval rules, Run grants and budget reservation again. Uncertain sent calls and continuations follow ADR 0073's Reconciliation rule.

An individual persistent sampling allow rule identifies the Agent, Agent Plugin name, server name, pinned-copy digest and origin. It applies to new Agent versions carrying that same copy and origin, but changing either requires a new allow rule. An individual deny rule instead follows the logical Agent, Agent Plugin name and server name across copy and origin changes, until the person removes it. This asymmetry keeps updates from silently extending a permission or removing a refusal. Explicit wildcard rules retain their stated scope.

## Protocol scope expanded on 2026-10-23

Issue #414 includes MCP 2026-07-28 support as well as the existing initialization-based protocol, over both stdio and Streamable HTTP. Agent Plugins 1.0.0 identifies the package format, separately from these wire-protocol revisions. The maintainer explicitly rejected deferring the modern protocol to a separate Issue.

The modern protocol carries sampling and elicitation in `InputRequiredResult` and resumes through `inputResponses` and an opaque `requestState`, rather than server-initiated JSON-RPC requests. Sampling remains within the requested feature scope despite its deprecated status in the modern specification. The Agent scope, pinned-copy authority, approvals, inference accounting and bounded human interaction are shared across both protocols.

Modern checkpoints preserve the original method, arguments, origin, Agent Plugin pin, requested inputs, answers and remaining cumulative waiting allowance. Aidash echoes the opaque state exactly and does not interpret it. Resumption rechecks existing live authority and approval/grant validity, and sends a continuation under the same logical Tool call approval. A checkpoint known to have been received permits advancing to its next round; an uncertain sent round is never replayed automatically.

The Deployment sets a continuation ceiling, defaulting to 10 continuation sends per logical tool call. It includes rounds carrying only `requestState`, so zero-human-wait loops are bounded too. Exhaustion is recorded and ends the tool call with a typed error. Lease changes, batches and continuation rounds never reset the cumulative human-wait allowance or per-sampling accounting.

Primary references: [version compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning), [multi round-trip requests](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/mrtr), and [sampling](https://modelcontextprotocol.io/specification/2026-07-28/client/sampling).
