# Agents create Subnodes, which are Run-scoped groups and not Nodes

Decided on 2026-10-31 for #660. ADR 0028 kept agents from creating Nodes and left a Subnode, one that never exceeds its creator's authority and ends with its parent Run, to a separate design. Agents now need to put together a group of agents for one part of a larger Task, with its own Workspace and a narrower authority than their own. A Cluster cannot do this: it is a Registry definition that people register and approve, not something an agent assembles mid-Run.

An agent the Tenant's policy permits may create a **Subnode** inside its own Node. A Subnode is not a Node. It has no identity, no address and no Node Registration, and it lasts until its creator dissolves it or the creating Run ends.

- **Members.** Its members are agent versions with a Catalog Approval on that Node. The creator names one of them the leader, and every Task given to the Subnode runs as the leader's Run, as a Cluster's work runs as its Coordinator's. The creator and the members may add approved agents later, under the same checks, and may remove members with no unfinished work.
- **Authority.** The creator passes an allowance of tools and Secrets. A member may use only what the creating Run, the allowance and its own definition all permit. A candidate member that needs anything outside this is refused when it is added. This is stricter than delegation, where a child may bind definitions its parent never bound (#401). Members hand work only to each other. They may write to the creating agent but cannot give it Tasks, because the creator acts with the wider authority the Subnode was meant to withhold. Approvals still go to the person at the root of the delegation chain.
- **One level.** A Subnode never creates another Subnode.
- **Workspace.** The Subnode has its own Workspace. The creating Run may read it, and the members cannot read the creator's. Once the Subnode ends, its Workspace stays read-only with the creating Run's records, so citations into it keep resolving.
- **Limits.** The Tenant's policy caps the members of a Subnode and the Subnodes a Run holds at once, and the spend counts on the creating Run's delegation chain. Cancelling the creating Run, or a Safety Stop on it, reaches its Subnodes.
- **Remote Grants.** A Run that a Remote Grant started may create a Subnode on the receiving Node only when a separate setting of the receiving Tenant allows it. It is off by default, and the receiving Tenant's limits and spend apply.

## Considered Options

- **Let agents create ordinary Nodes.** A created Node would hold an identity, an address, Node Registrations and a share of the Tenant's Node limit beyond the Run that made it. Rejected.
- **Give a Subnode its own identity, the Subnode of ADR 0028.** A Run-scoped party in Federation could not be held to its creator's authority. Rejected.
- **Let members run agents defined on the fly.** Only versions that a person approved run in a Node. Agent-written definitions belong with Workbench drafts and their approval. Rejected.
- **Let members delegate outside the Subnode, or give Tasks to the creator.** Either route would carry work past the narrowed allowance. Rejected.
- **Call it a Crew.** The name would leave ADR 0028 unchanged. The term Subnode was kept and redefined so that the deferred design and this one read as one line of thought. Rejected.

## Consequences

- ADR 0028's "Agents cannot create Nodes (Subnodes)" now reads "Agents cannot create Nodes". The Subnode it deferred is the one defined here.

## Amendment (2026-11-06)

A creator could not hand work on after giving its Subnode a Task, and members could not use the Task it gave them (#750, #755). What the creator gives a Subnode is recorded as one of its reads, and a Read Recheck under an inherited authority stopped at any Workspace but its Run's own.

- **A Read Recheck crosses the boundary; a read does not.** While it checks a Run's reads, a Read Recheck reaches the Subnode Workspaces that Run created or works in, whoever is checking. That covers a Run the creator delegated to, the creator's parent, and Subnodes that have ended. Policy still decides each record, and only the creating Run reads that Workspace directly.
- **Only members skip the creator's reads.** A Run working in a Subnode's Workspace does not check what the creating Run had read, because it cannot read those records and the creator chose what to hand over. Anyone else who opens that Task is still checked, so a person who may not read the creator's records stays refused on it. So withdrawing something the creating Run read does not stop its members, though the creating Run and everyone else lose sight of that Task. A person stops the members by cancelling the creating Run or dissolving the Subnode.
- **A Read Recheck judges a definition by current policy.** An inherited authority still uses only the definitions its Run already used. A Read Recheck hands nothing over, so it no longer needs that: it decides as an authority that is not inherited does, and what the Run may use does not grow. Without this a creator could not read its members' outputs, and a parent could not depend on a Task its child created, so the rule applies to every inherited authority. It takes no lock, so an approval withdrawn at that moment can pass as if the read came just before it.
- **The boundary also holds from member to creator (#793).** A member's message to the creating Run stays bound to what the member read: it is recorded as the member Run's output and as a read of the creating Run, so withdrawing what the member read hides the message from every reader and stops the creating Run that received it.

Not chosen:

- **Add the creator's Subnode Workspaces to its own authority.** A Run it delegated to would still stop at the same boundary.
- **Skip the creator's reads for every reader.** People could then see what they may not read at its source.
- **End the Read Recheck at the Subnode's boundary.** Members may bind Sources the creator did not, and the check on those would be lost.

## Amendment (2026-11-07, #759)

Once a Subnode ends, an Agent working in its Workspace can no longer write there, apart from what stays open below. It cannot post a message with files, copy a Thread Attachment into its Working Area, change Memory, ask a person a question, open an approval, consume an approved Web disclosure or start a Live preview. A call that already passed the tool's entrance is refused when it writes: the write takes a shared lock on the Subnode, so the end waits for a write in progress and a refused call leaves nothing behind. The Run is cancelled at the same moment, so a refusal gives no reason.

What stays open: recording how an approval was settled, a Run's final completion and the Completion Gate check, and reading Memory. Each only closes out or reads what exists, and stopping any of them could keep a Run from ending. Stopping, replacing and settling a Live preview stay open for the same reason, and so that a Pod can still be stopped. Two requests to a person also stay open, because each closes out or mirrors something that already exists: the confirmation request that closes out a tool call whose outcome is UNCERTAIN, and the copy of a request that the home Node of a remote Run already recorded.

A person can still correct or delete the Memory an Agent left in the Workspace, unlike messages and files. That write does not race with the end, and a person must be able to remove what is left there.
