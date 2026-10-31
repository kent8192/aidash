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
