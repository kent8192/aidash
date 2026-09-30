# Remote semantic memory for Issue #75

Status: accepted on 2026-09-30. Decisions Q1-Q12 and the assembled specification received final shared-understanding confirmation. Implementation and runtime acceptance are not complete.

Issue: [#75](https://github.com/kent8192/aidash/issues/75). Target: `develop/0.1.0`, milestone `v0.1.0`.

The [specification and acceptance matrix](2026-09-30-remote-semantic-memory-specification.md) assemble these decisions into a reviewable execution and recovery contract.

## Requirements and evidence

The canonical [functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24), retrieved during this interview with a page edit timestamp of 2026-09-28T12:03:07.241Z, require FR-MEM-001 and its integration in section 12. They do not explicitly require every remote Agent to retrieve semantic memory. The supported path must be stated and tested; skipping retrieval is not evidence that retrieval succeeded. [Issue #67](https://github.com/kent8192/aidash/issues/67) remains the place for release-scope changes.

Source inspection is pinned to `827480c13d796bca142787be5bbd25ce34c80551`. It is not a runtime acceptance result:

- [`Guard::semantic_context`](https://github.com/kent8192/aidash/blob/827480c13d796bca142787be5bbd25ce34c80551/src/authorization/execution.rs#L1086) returns `Ok(None)` for a remote guard before performing retrieval.
- [`Harness` context construction](https://github.com/kent8192/aidash/blob/827480c13d796bca142787be5bbd25ce34c80551/src/harness.rs#L930) consumes the optional result as `semantic_memory`.
- The local path records source revisions in `semantic_run_reads` before inference. The current remote grant read ledger does not establish equivalent remote semantic dependencies.
- [`docs/semantic-memory.md`](../semantic-memory.md) describes local retrieval, source revision checks, bounded context, deletion and current source authority, and explicitly leaves scoped semantic federation at the integration boundary.
- [`src/generation/embedding.rs`](../../src/generation/embedding.rs) discovers generated ancestors from the current store's generation records and node-qualified subjects. Invoking the Home implementation alone does not prove enforcement of an execution-node generated Agent's allowances.
- The glossary distinguishes Logical Agents from Agent definitions. Current semantic source matching uses a node-qualified Agent ID and definition version; Q4 explicitly preserves that boundary for the selected path.

## Accepted decisions

### Q1: Home Workspace ownership

Remote execution retrieves from the Task's Home Workspace. The candidate scope is the Home tenant and Workspace, including shared sources and sources authorized for the executing Agent. The execution node's own memory and automatic unions of multiple memory stores are outside this selected path. This decision does not confer permission to read any source.

Rationale and alternatives: [ADR-0010](../adr/0010-home-owned-remote-semantic-memory.md).

### Q2: Ordinary and generated remote Agents

The Issue #75 acceptance matrix includes both an ordinary Agent retrieving during remote execution and an automatically generated Agent itself retrieving during remote execution. A local generated-Agent test plus an unrelated ordinary remote-Agent test does not establish that combination.

Retrieval applies to executions for which it is enabled. It is not forced on every Agent. Q6 binds enablement to the remote grant; Q9 resolves inference boundaries and Q11 resolves generated-Agent origin.

This selected path supplements the existing FR-MEM-001 and section-12 gates; it does not remove their other requirements or constitute acceptance of the entire release.

### Q3: Explicit retrieval outcomes and failure

An enabled retrieval must not silently continue inference without memory after failure. Backend failures permit bounded retries; authority or credential revocation prevents the next inference and unauthorized access to dependent information. Insufficient context budget to preserve even the result provenance is an explicit budget failure.

A successful authorized search with zero matches is a valid empty result and can continue. Intentionally disabled retrieval, zero matches and failed retrieval are distinct outcomes. Q7 resolves dependency invalidation and authority restoration; Q10 resolves bounded retries.

### Q4: Exact-definition Agent scope

The Agent-specific boundary is the combination of Home tenant, Home Workspace, Agent owning Node, Agent ID and exact definition version. Authorized Runs using that same definition may share these sources. Another Node's same-named Agent or another version has no automatic inheritance. Shared Workspace sources remain eligible under current source permissions.

This is not private memory for each Logical Agent. New logical-participant identity or automatic memory migration across versions is not introduced by this decision. Remote implementation must use the actual executor's qualified identity rather than accidentally qualifying its ID with the Home node.

### Q5: Home provider and origin-owned generation allowances

Home invokes the Workspace index's embedding provider and owns its service credential and financial bill. The execution node receives bounded results and provenance. Subject and provider bearer values are not forwarded.

A generated Agent's policy must explicitly approve the Home provider, exact version and configuration. Every relevant generated ancestor retains its originating permission and budget authority; applicable call and shared-token reservations must be durable before provider dispatch, including where that authority belongs to the execution node. Failed or uncertain dispatched calls remain charged. Invocation at Home cannot turn a generated query into an ordinary unmetered query.

See [ADR-0011](../adr/0011-home-embedding-with-origin-owned-allowances.md). Q11 resolves the generation origin; the accepted specification describes the lineage binding and reservation protocol.

### Q6: Explicit disclosure and both-node authority

Local Home search permission alone does not permit disclosing source text to a remote executor. The remote grant binds retrieval enablement, destination Node, Agent version and inference model, with explicit semantic disclosure permission in addition to current Home search/read permissions. The execution node must independently permit receipt and use for the mapped identity and executor.

Existing remote execution grants must not silently gain semantic access. Current authority is required at subsequent use and read boundaries; a previous admission is not perpetual permission. Q12 extends explicit disclosure approval to context compaction.

### Q7: Invalidation, visibility and recovery

Deletion or revision change of an observed source stops dependent Runs. Work using updated sources starts a new Run without importing the invalid context. Replacing only the current search result cannot establish removal of that source from prior model output or summaries.

Dependent journals, Artifacts and Messages are withheld through both Nodes' read paths. Audit records are access restricted and governed by the separate [retention requirements](../operations/nonfunctional-release.md); this decision does not authorize indefinite retention or promise remote erasure of already delivered data.

Temporary infrastructure failure can recover after current authority and dependencies are checked again. Restored authority requires explicit authorized resumption and full revalidation of the original bindings and dependencies. A revoked original credential or replaced grant cannot be silently rebound into the old Run. These rules also apply after either Node restarts.

See [ADR-0012](../adr/0012-invalidate-runs-with-changed-semantic-sources.md).

### Q8: Read-only remote path

The selected remote path searches and reads Memory, Message and Artifact sources ingested at Home. It does not introduce remote semantic ingestion, source mutation, deletion, reindex management or `memory_write`. Remote `memory_write` is explicitly unavailable; it must not silently save an execution-node substitute. Existing authorized Home management and local indexing retain their own contracts.

This makes the supported scenario and restriction explicit under Issue #75. It does not remove the FR-MEM-001 ingestion/update/deletion/reindex requirements at Home or reduce the release requirements through Issue #67.

### Q9: Retrieval at each inference boundary

Each inference using task context performs semantic retrieval from the current Task and that Run's authorized inputs. The current authority, source/index state and remaining context budget govern the result.

Re-delivery of the same operation can reuse a durable result only after checking current authority and source/index state again. A later inference is a new retrieval operation; it does not inherit an unchecked cached result. Previously consumed dependencies remain part of the Run even when later retrieval returns different matches.

### Q10: Bounded automatic retry, then explicit resumption

A transient backend or transport failure permits up to five automatic retries after the initial attempt, with waits of 2, 4, 8, 16 and 32 seconds. Exhaustion pauses the Run for explicit authorized retry rather than silently continuing inference or automatically completing the Task as failed. Grant expiry, cancellation and remaining generation allowances can stop retries earlier.

Retry state and consumed budgets survive both-node restarts. Completed-request re-delivery must not charge another provider invocation. A dispatched attempt with uncertain outcome remains charged, and a new invocation requires a new reservation. Authority denial, invalid provider output and exhausted budgets are not automatically retried. Explicit retry does not restore consumed allowance or replace invalid bindings.

### Q11: Generate at the execution node for the Home Task

For the selected generated remote scenario, Node A owns the Task, while Node B generates and runs the Agent under B's generation policy. B owns its generation record, lifetime, permission and resource allowance, with an explicit durable link to A's Task and subsequent remote grant.

Any generated delegators at A still constrain the assignment and its usage. Copying a definition or omitting a remote generated ancestor must not convert the execution into ordinary unmetered work. The specification separates generation preparation from activation to avoid a circular prerequisite between creating the Agent and creating its exact-definition execution grant.

See [ADR-0013](../adr/0013-generate-remote-agents-at-the-execution-node.md).

### Q12: Explicit approval for context compaction

An external context-compaction provider that receives semantic context or derived history requires its own approved destination, version and configuration. Home disclosure permission, receiving-node authority and every applicable generated ancestor's compaction permission and call allowance apply before dispatch.

Without that approval, no external compaction request is made. Inference can continue if the context fits without compaction; otherwise the Run pauses with an explicit context/compaction reason. Inference-model approval cannot authorize another processor implicitly, and generated chains cannot fall back to an environment-selected provider.

## Design tree

| Decision                                    | State         | Contract location                                       |
| ------------------------------------------- | ------------- | ------------------------------------------------------- |
| Home memory ownership                       | Accepted: Q1  | ADR-0010; specification scope and identity              |
| Supported remote executions                 | Accepted: Q2  | Ordinary/generated acceptance cases                     |
| Failure versus empty results                | Accepted: Q3  | Retrieval outcomes and failure table                    |
| Agent-scoped memory identity                | Accepted: Q4  | Exact executor identity and definition isolation        |
| Cross-node disclosure and current authority | Accepted: Q6  | Grant bindings and execution boundaries                 |
| Embedding provider and usage ownership      | Accepted: Q5  | ADR-0011; reservations and settlement                   |
| Source invalidation and dependent outputs   | Accepted: Q7  | ADR-0012; dependency visibility and recovery            |
| Remote memory writes                        | Accepted: Q8  | Explicit read-only remote contract                      |
| Retrieval timing                            | Accepted: Q9  | Per-inference operation and replay rules                |
| Retry and exhaustion                        | Accepted: Q10 | Durable schedule and manual resumption                  |
| Generated remote origin                     | Accepted: Q11 | ADR-0013; generation preparation and activation         |
| Context-compaction disclosure               | Accepted: Q12 | Processor approvals and budget checks                   |
| Assembled protocol and acceptance matrix    | Accepted      | Companion specification; all design decisions confirmed |

## Documentation boundaries

This document records confirmed decisions. The glossary contains domain terms only. ADRs record consequential trade-offs. The accepted companion specification defines the protocol and verification obligations. Existing runtime documentation continues to describe implemented behavior until implementation and verification establish the new contract.

The repository ignores new files under `docs/adr/`; the new ADRs are retained locally alongside this design without changing ignore rules. No implementation, commit, push, GitHub/Notion update or PR is represented by this design session.
