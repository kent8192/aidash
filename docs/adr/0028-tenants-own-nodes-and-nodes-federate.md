# Tenants own Nodes, and every Node-to-Node exchange is Federation

Until now one Deployment was one Node: a single federation identity whose
Tenants shared one Registry. Agents in two Tenants of the same Deployment could
not reach each other at all, but the same agents could cooperate across
Deployments through peer mappings (which rejected `source_node == node`). Where
an agent could cooperate depended on where it was installed, not on who agreed.

The hierarchy is now Deployment, then Tenant, then Node. A Node is an agent hub
inside one Tenant with its own identity. Its subjects, Workspaces and Runs live
in it, and execution happens inside it. Communication between any two Nodes is
Federation: the sender is identified, the receiving Node must have explicitly
registered it, and the receiver's authority decides. This holds within one
Tenant, across Tenants and across Deployments.

## Considered Options

- **Keep one Node per Deployment and allow same-Deployment peer mappings.**
  This closes the asymmetry, but agents keep no identity below the Tenant, and
  Federation stays a Deployment-level concept. Rejected.
- **Let a Tenant span Deployments.** Policies and revocations would have to be
  synchronized across independently operated databases, and it would be unclear
  which side's decision is authoritative. Rejected.

## Consequences

- The former Node, the whole installation's identity, is renamed Deployment.
- Federation's identities, peer configuration, `home_node` and the protocol's
  node identifiers move from the Deployment to the Node. This rebuilds the core
  of Federation.
- A Tenant never spans Deployments.
- User subjects (people) belong to the Tenant and work in any of its Nodes.
  Agent and service subjects belong to one Node.
- A Node starts as a logical boundary of identity, authority and data, sharing
  the Deployment's workers and per-Run Execution Pods. Compute dedicated to a
  Node can be added later as an option.
- Within one Tenant, its policy may admit the Tenant's other Nodes without
  per-Node registration. Across Tenants, explicit registration by the
  receiving Node stays required. The receiver's authority decides in both cases.
- Agents cannot create Nodes. Parent tasks, the delegation chain and clusters
  cover agent-made structure. A Subnode that never exceeds its creator's
  authority and ends with its parent Run was left to a separate design (ADR 0110).

## Status

Amended 2026-10-18 by ADR 0064: Federation is authenticated by Deployment Keys, and a Node Registration by the receiving Tenant's administrators replaces Peer Credentials.

Amended 2026-10-31 by ADR 0110: agents may create Subnodes, Run-scoped groups of agents that are not Nodes.
