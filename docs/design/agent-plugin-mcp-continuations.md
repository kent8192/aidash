# Agent Plugins remaining-work grilling (2026-10-23)

Issue: https://github.com/kent8192/aidash/issues/414

The accepted Q1-Q49 decisions, as incorporated and amended in Issue #414 and ADR 0072-0074, remain the baseline. Their current form takes precedence over historical implementation briefs; for example, Run-start tools are pinned in Context.plugin_tools rather than changing the admission Binding snapshot. This continuation records new decisions and explicit amendments. The user invoked `grill-with-docs`; the user accepted the recommendations, included the modern protocol in Q66, and authorized implementation and a PR on 2026-10-23.

## Accepted decisions

The user answered "all recommended" to Q50-Q55, Q56-Q61, Q62-Q65 and Q67-Q70.
For Q66, the user explicitly included the modern protocol in scope instead of accepting the recommendation to defer it.

- Q50: Resolve stdio dependencies at attachment in a time-bounded preparation operation under the attaching person's authority. Check the draft Agent's package-index Bindings and Tenant Catalog Approvals; issue a short-lived gateway token confined to that preparation. The prior rule to skip and report missing prerequisites remains.
- Q51: A sent MCP call with an unknown outcome uses existing Reconciliation. Only a request established to be unsent may be sent after reconnecting. Read-only declarations do not change Unsafe replay behavior.
- Q52: Add a Deployment ceiling for MCP human waits, default 15 minutes. Use the shorter of that ceiling and approval expiry; Elicitation also respects Answer Deadline. This explicitly amends the former approval-expiry-only ceiling, whose implemented default is now 24 hours.
- Q53: A Run processes at most one sampling or elicitation request at a time. Reject overlapping requests instead of queuing them. A subsequent request may start after the preceding reply.
- Q54: Preserve the last validated, atomically saved `${PLUGIN_DATA}` version. Cleanup may recover and validate newer data after an abnormal exit. A lost Pod, invalid data, quota violation or failed save preserves the previous version and records a Run event.
- Q55: Close a quiet remote MCP session on Run pause or idle. Reconnect on the next use and check pinned tool schemas. Keep an open call's sampling/elicitation session alive within its waiting limits.
- Q56: Preparation has a separate bounded concurrency quota from Workbench tests, with Deployment ceilings and Tenant settings within them. Use the Runner's installation time limits; report exhausted capacity to the attaching person.
- Q57: Accumulate human waiting within one outer tool call. Subsequent sampling/elicitation requests get only its remaining allowance, also bounded by their approval expiry and any Answer Deadline.
- Q58: If sampling cannot reserve the Run's token budget or its provider call fails, reply to that sampling request with an error and let the usable session's outer tool call continue. Keep existing reservation, refund and unknown-usage accounting. Session loss follows Q51.
- Q59: One Inference Attempt per server sampling request, with no automatic retry. A later request undergoes the normal approval/rule/grant decision and budget reservation again.
- Q60: An individual persistent sampling allow rule targets Agent ID, Agent Plugin name, server name, pinned-copy digest and origin. Reuse it across Agent versions carrying that same copy and origin; a changed copy or origin needs a new allow rule. Explicit wildcard rules retain their stated scope.
- Q61: An individual sampling deny rule targets Agent ID, Agent Plugin name and server name across copy and origin updates, until the person removes it. Explicit wildcard rules retain their stated scope.
- Q62: All stdio servers of one Agent Plugin in a Run share one live `${PLUGIN_DATA}` area; 64 MiB applies to their combined contents. Preserve Q54's persistence boundary. Pod grouping is an implementation choice subject to this shared view and Run/Agent Plugin isolation.
- Q63: Freeze writers and publish Working Area changes at each completed stdio tool-call boundary. If another tool advanced the revision, restart affected sessions against that revision before their next call, carrying forward saved Agent Plugin data and reporting the reset. `${PLUGIN_DATA}` persistence remains distinct from Working Area publication.
- Q64: Pin dependencies for the supported Linux amd64 and arm64 Runner targets, recording CPU and Node/Python compatibility information. An incompatible receiving environment skips the affected stdio server with a reason. Re-resolution requires a new Agent version.
- Q65: Per-server Tenant Secret bindings explicitly select raw or Bearer header form; Authorization defaults to Bearer and other headers to raw. Replace any package header with the same case-insensitive name. Transport-owned names remain invalid. Catalog Approval shows the effective name/form without the Secret value, retaining origin restriction and current-Secret checks.
- Q66: Include MCP 2026-07-28 modern protocol support in #414 alongside the existing legacy protocol, over both stdio and Streamable HTTP. Agent Plugins 1.0.0 is the package-format version. The proposed separate-Issue deferral was rejected. Preserve the accepted feature scope, approvals and accounting; modern wire behavior requires the next decision frontier.
- Q67: After a received modern InputRequiredResult, persist its acknowledged continuation, answers and cumulative deadline; release the worker lease while waiting for a person. Retain stdio Working Area ownership and freeze Pod writers. Resume the same logical invocation and original Tool call approval, preserving its method, arguments, origin and pin. Legacy retains its lease/session wait. Uncertain sent continuations follow Q51.
- Q68: Accept bounded modern input-request batches, displaying and executing one input at a time. Send a continuation only after all required results are ready. Preserve legacy overlapping-request refusal and apply cumulative human-wait and per-sampling budget/attempt limits throughout the batch.
- Q69: Modern sampling refusal, expiry, budget failure or provider failure ends the logical tool call with a typed tool error and sends no continuation. Do not fabricate a completion or custom error envelope. Legacy retains Q58's error reply to its live server. Elicitation decline/cancel are valid results and may continue normally.
- Q70: A Deployment-configurable ceiling defaults to 10 continuation sends per logical tool call, including state-only rounds. Record exhaustion and finish with a tool error. Preserve the cumulative human-wait ceiling and per-sampling accounting.

## Documentation

- ADR 0072: distinguish the existing Shared Package distribution from the separately designed central Marketplace.
- ADR 0073: preparation authority and capacity, uncertain calls, session/data lifecycle, Working Area synchronization, dependency compatibility and Secret header composition.
- ADR 0074: dual-era protocol scope, cumulative human waits, durable modern checkpoints, serial batches, protocol-specific failure behavior, continuation limits and persistent rule identity.
- CONTEXT.md: Agent Plugin preparation and Agent Plugin data; applicable waiting bounds in Sampling approval and Elicitation definitions. Operational details stay in the ADRs.

## Remaining design frontier

All Q50-Q70 decisions have been answered, including the explicit Q66 scope expansion. The final consistency audit found no remaining design questions. The user confirmed the shared understanding and authorized implementation and a PR on 2026-10-23.

## Confirmed shared understanding

- Deliver both legacy and MCP 2026-07-28 behavior over remote Streamable HTTP and stdio, preserving immutable Agent-scoped copies, Run-start tool pins and the existing feature restrictions.
- Integrate actual Run and Workbench dispatch, public-address enforcement, per-call remote approval/Read-only checks, and per-server origin-bound Tenant Secret headers.
- Use bounded, separately limited author-scoped preparation; pin and install dependencies only through the Egress gateway for supported Runner targets.
- Share live Agent Plugin data across a plugin's stdio servers; keep its durable saving separate from logical-call Working Area publication and safe session resets.
- Apply cumulative human waiting (default 15 minutes), one active input and one Inference Attempt per sampling request, per-server approvals/grants/rules and existing usage accounting.
- Keep legacy wait sessions and leases alive. Persist acknowledged modern checkpoints and release human-wait leases, with stdio writers frozen; process modern batches serially and permit at most 10 continuation sends by default.
- Never automatically replay an uncertain sent call or continuation. Modern sampling refusal/failure ends the logical call locally; valid elicitation decline/cancel may continue.
- Complete API/contracts/docs and real Self-hosted execution acceptance. The central Marketplace and Cloud/Dedicated #310 scope remain separate; coordinate missing session-lifetime authority with #405 while reusing the already merged #461 foundations.

The confirmation closed the design interview. Implementation and verification evidence are recorded separately from this accepted design.

## Existing contracts to inherit

Run grants retain their exact Run/Agent/subject/destination scope and expiry; stale or expired answers cannot revive a closed request; concurrent answer/expiry resolution is atomic. These do not require reopening the accepted decisions.

Writer fencing and confirmed termination before successor publication also apply to Agent Plugin data. Dispatch resolves the current active Tenant Secret and checks its permitted host; a long-lived session cannot cache an obsolete Secret indefinitely. Run cancellation and loss of execution authority stop in-session work under existing rules, preserving unknown usage and rejecting stale human replies.

Preparation results are adopted through existing author permissions and draft expected-revision checks; completing late does not authorize overwriting a changed draft. Unresolvable dependencies use component-level skip-and-report. Out-of-session sampling/elicitation is outside ADR 0074's accepted scope and is refused without starting an approval or inference.

The original Tenant-to-Tenant Agent distribution carries the immutable copies and lockfiles. Its new name is Shared Package in the separately designed Marketplace terminology. Central Marketplace distribution, Entitlements, Declared Permissions and paid distribution remain the scope of that separate design; they do not expand #414's implementation scope.

## Derived implementation acceptance

These checks define acceptance for the whole Issue, including the #405-dependent stdio phase. The Draft PR records the subset executed for its delivered remote and client-input paths.

- Preserve Phase 1 imports, schema/component results, pinned copies, direct Skill behavior, Tenant isolation and approval disclosure.
- Connect both protocol eras to actual Runs and Workbench; verify negotiation, Run-start pinning, aliases, allowlists, exposure, Read-only evidence/digests, Tool call approvals and schema-change errors. Verify uncertain calls and continuations are not replayed.
- Integrate the shared public-address policy, including approved private origins, every new connection's DNS checks and disabled redirects. #376 is being delivered separately in PR #466.
- Exercise legacy in-session waits and modern durable continuations, Sampling approvals and Thread Elicitation, grants/rules/modes, cumulative expiry, legacy overlap refusal, modern serial batches and the continuation ceiling. Verify one Inference Attempt per request, accepted-response reuse after durable resume and failed/unknown usage. Workbench denies and records both request types in each protocol era.
- Exercise bounded author-scoped dependency preparation and lockfile installation through the existing gateway, including missing approvals, integrity failures and per-environment compatibility.
- Exercise stdio in an actual Self-hosted gVisor Execution Pod, session-lifetime gateway authority, pause/idle/restart, shared Agent Plugin data and follow-up Runs. Verify Working Area publication at call boundaries and safe data recovery/fencing.
- Preserve remote-admission refusal and receiving-Tenant approvals when distributing Agent versions with their copies. Cloud/Dedicated package installation and Secret forwarding remain dependent on #310.
- Update OpenAPI, Bruno contracts and registry-capabilities operations documentation for the delivered behavior. Distinguish local checks, hosted CI and real execution acceptance in completion evidence.

#461 supplies the merged Tenant Secret, operation-egress, package-index and Runner foundations. Session-lifetime authority still needs coordination with #405; retain the original dependency order instead of assuming the existing operation token already satisfies it.

Primary sources verified on 2026-10-23: https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning and https://modelcontextprotocol.io/specification/2026-07-28/client/sampling. The latter marks sampling deprecated in that revision. At the start of the interview the locked dependency was rmcp 1.2.0, whose ProtocolVersion::LATEST is 2025-06-18. The remote Agent Plugin adapter now uses rmcp 3.7.0; Registry MCP tools retain their existing SDK.

Additional primary evidence: https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/mrtr and https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2026-07-28/schema.ts (InputResponse at lines 541-542; matching input keys at lines 600-605). requestState is opaque and must be echoed exactly; its server-side replay protection does not justify retrying an uncertain sent request.

SDK implementation evidence: official rmcp-v3.7.0 exposes call_tool_once(), while its automatic MRTR driver uses try_join_all for input requests. Aidash must drive continuation checkpoints and serialize inputs to preserve the accepted contracts rather than adopting that parallel automatic path: https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.7.0/crates/rmcp/src/service/client.rs.


## Delivery boundary

The existing #414 branch and this draft deliver remote sessions, public endpoint checks,
Run-start tool pins and per-call approval, plus legacy in-session client inputs and
2026-07-28 MRTR checkpoints. Sampling has a separate approval kind and per-request
Inference Attempt; form elicitation is answered from the Thread. ADR 0072-0074 remain
the accepted design for the whole Issue.

Issue #414 remains open. Phase 4 follows #405's session-lifetime egress authority:
`mcp_stdio`, sandboxed stdio sessions, attachment preparation, platform lockfiles,
session installation, shared/persisted PLUGIN_DATA, Working Area publication and
remote-server Tenant Secret binding. #310 still gates Cloud/Dedicated egress and
Secret access. Attachment-time Read-only listing receipts and their editor also
remain to be integrated; Run-start schema-digest checks are already enforced.
Workbench currently offers remote Agent Plugin tools through fixtures; real-tool
profile dispatch and its explicit denied sampling/elicitation receipts remain
to be connected.
This draft must not claim these acceptance criteria or close #414.

The local execution evidence uses the development foundation merged at
`4a9a23497c91418b1757f950660a1a1598f0bfdb`. During implementation, #444 merged the
Tenant-owned Node and Federation identity changes into `develop/0.1.0`. Forward
integration of that foundation is still required before this draft can become
merge-ready; preserve the planned forward-merge step rather than treating the
older local execution evidence as acceptance of the new Node model.
