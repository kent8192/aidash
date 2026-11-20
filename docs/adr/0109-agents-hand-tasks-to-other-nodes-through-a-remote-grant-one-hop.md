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

## Amendment (2026-11-17)

A Remote Grant now carries the Home's Federation Tool Request as an RFC 9396 `authorization_details` entry, and the receiving Node reads the current Request each time it describes the grant (ADR 0139). The grant still lasts one hour, but the Home renews it every hour while its Run lives and the delegation chain still holds, so a Run that waits for an answer or works for long is not cut off at the hour. `task_assign` from such a Run is served by the Home and assigns only the Home Node's own or generated Agents, so it never adds a hop.

## Amendment (2026-11-19, multi-hop delegation, #654 via #1013)

ADR 0143 removed the one-hop limit. A Run that a Remote Grant started may call `agent_discover` and `task_delegate`, and the work it hands on may go to any Node, including the Home's own Node and the receiver's own Node.

- **Every hop is granted from the Task's Home.** The Run's child Tasks live at the Home, so `task_delegate` is the Home's scoped `delegate` command. For another Node the Home checks the child under the command's lease, releases its locks, and prepares and activates a Remote Grant of its own for the child under the grant's whole chain, never as its root person alone. `agent_discover` is the Home's scoped `discover` command, which returns only cards.
- **Authority.** The Home judges the whole chain by its own policy, as for any grant it prepares: every subject of the chain, the asking receiver's executor included, must be allowed `task.delegate` on the child and `federation.execute` on the target, and a prepared grant is reused only for exactly that chain. Only the new receiver's executor may be unknown to it. An earlier receiver's executor must be an Agent subject of the Home's policy, as it already must be for the Home to admit a Task that executor created; otherwise the hop is refused.
- **Consent.** The middle Node consents through its own policy (`tool.invoke` on the tool and `task.delegate` on the Home's Task as a foreign resource) and the Agent's `allow_task_delegation`. It names only a Task the Home records as the grant's output, judged with that Task's own attributes. Each target decides by its own Node Registration, Peer Mapping, policy and Catalog Approval.
- **Federation tool lists.** Across Tenants a remote Run uses `agent_discover` and `task_delegate` only when the Home's Federation Tool Request and the receiving Node's Federation Tool Allowance both name them (ADR 0139). Their Reach is the Home's Workspace, because the Home serves both commands; they are not among the default tools, and the Home checks its own Request again before it serves `discover` or `delegate`.
- **Data Protection.** A Run admitted from another Node, or working a Task another Node's Run created, already assumes the strictest policy (ADR 0104), so nothing travels with the chain.
- **Limits.** There is no depth limit. Every hop creates a new Task that can be assigned once, so the same Task cannot loop. Budgets bound what the chain spends.
- **Results and Thread Attachments** reach the Home directly from every hop; a Run fetches Thread Attachments only from its direct Home.
- **Lifetime.** A Remote Grant whose activation the Home recorded and whose receiver holds its Task (the Task's owner is the receiver's executor) lives while that Task is open, however long the Run sleeps, and ends with the Task; its 1–3600 second lifetime bounds only the admission (#1013, Round 3 Q5). The hourly renewal of the 2026-11-17 amendment and the Home's tool asks use the same rule, not a second one: the renewal extends only a grant whose activation the Home recorded, whose receiver holds its Task, whose Task has started and not ended, and whose chain holds. A grant never activated is never renewed and expires with its lifetime; a started grant the renewal no longer extends stays live under this rule, and every use still re-evaluates the whole chain.

Considered and rejected: relaying through the middle Node, which would store another Node's Task there and carry results and Thread Attachments through every hop; and stripping every executor the Home does not know from the chain, which would let a Node the Home never named act in its chains unchecked.
