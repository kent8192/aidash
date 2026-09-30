---
status: accepted
---

# Keep remote semantic memory in the Home Workspace

For [Issue #75](https://github.com/kent8192/aidash/issues/75), remote execution searches semantic memory owned by the Task's Home Workspace, within that Home tenant and Workspace and the executing Agent's authorized scope. This keeps knowledge ownership aligned with the shared work and its source authority; execution-node memory and a combination of both nodes' stores would add separate disclosure, revocation and accounting contracts. The selected v0.1.0 path therefore uses Home memory only; the detailed decisions are in the [design record](../design/2026-09-30-remote-semantic-memory.md).

Agent-scoped sources retain the existing exact-definition boundary: the Agent's owning Node, ID and version within the Home tenant and Workspace. Runs using that same definition may share authorized sources; a matching name on another Node or a different version does not inherit them. This is deliberately not a claim of private memory for each Logical Agent.

Cross-node retrieval also requires explicit semantic disclosure permission bound to the remote grant's retrieval mode, destination Node, Agent version and processing recipients, alongside current Home read/search permissions and current receiving identity/executor permissions. The inference model and any external context-compaction provider have separate approvals. Peer trust and an existing remote execution grant alone must not acquire this extra data access.

The selected remote path is read-only: it searches Memory, Message and Artifact sources ingested at Home. Remote `memory_write` remains explicitly unavailable. Adding cross-node writes would require a separate mutation and indexing contract, including how a Run avoids invalidating its own previously consumed sources.
