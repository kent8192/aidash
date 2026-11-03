# Agents hand Tasks to agents on other Nodes through a Remote Grant, one hop

Decided on 2026-10-31 for #649. `agent_discover` listed agents on the Tenant's other Nodes and on registered Nodes of other Tenants, but `task_delegate` refused every `node_id` except the caller's own Node. Its Rejected Call also told the model to pass "the node_id that agent_discover returned", which the model had already done. Only a person could hand a Task to another Node, through `POST /api/tasks/{id}/delegate` or `/remote-grants`. A second Node of a Tenant was therefore unreachable for agent work, which contradicts the rule that a Node talks to every other Node the same way.

When `task_delegate` gets another Node's `node_id`, it now creates a Remote Grant and hands the Task over. This is the flow a person uses: the grant lives for 3600 seconds, and a prepared grant for the same agent is reused. The tool reaches every target that `agent_discover` returns, whether it is in the same Tenant or in another Tenant.

The checks are the same as for local delegation:

- The Agent's `allow_task_delegation` setting.
- `task.delegate` and `task.execute` on the Task, for the delegation chain.
- `federation.execute` on the target Node.

The receiving Node decides by its own Catalog Approval and authority. If the policy asks, the Run waits in an Authority Pause. If it denies, the call is a Rejected Call, and that Rejected Call gives the real reason.

A Run that a Remote Grant started cannot discover agents or delegate another Task, including to its own Node. Those calls report the one-hop limit. Multi-hop delegation is #654.

The grant records the Run's delegation chain followed by the selected receiver's qualified Agent subject. The source evaluates the sending chain against its own policy; the receiver evaluates its own executor subject, Catalog Approval, and policy. The worker releases its retained transaction before peer I/O and revalidates the Run's authority at the source boundaries and again after the call.

## Considered Options

- **Keep remote delegation for people, and list only reachable agents.** A Cluster could not span Nodes, so adding a Node would split a Tenant's agents into islands. Rejected.
- **A separate remote-delegation tool.** It does nothing that `task_delegate` cannot do, and models would pick the wrong tool. Rejected.
- **A dedicated setting that allows agents to delegate remotely.** Policy over `federation.execute` already controls this. A second switch would decide the same thing in two places. Rejected.
- **Unlimited hops up to the depth limit.** Each receiver would have to weigh the authority and Data Protection Level of every Tenant earlier in the chain. Deferred to #654.

## Consequences

- The `task_delegate` description finally matches its behaviour (#572).
- Policy and Catalog Approval refusals record a decision, as do unreachable targets and incompatible executor subjects. The asking Node receives a limited reason from the receiver.
- A Tenant that wants to keep its agents from delegating across Nodes denies `federation.execute` to them in its policy.
