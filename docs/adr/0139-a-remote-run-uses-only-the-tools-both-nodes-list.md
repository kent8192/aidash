# A remote Run uses only the tools both Nodes list, and asks the side that lacks one

Decided on 2026-11-17 for #989 and #959. One boolean on each tool contract, `remote_exposure`, decided whether a Run that a Remote Grant started was offered a tool. It was set when tool contracts were introduced (#104) and never revisited, neither Tenant could change it, and nothing enforced it after a Binding was built: the runtime filter had no caller, so tools added during a Run, such as `journal_*` after an offload, reached remote Runs unchecked, while `subnode_*`, `agent_discover` and `task_delegate` were offered and always refused. A librarian Agent could not read its own Reference set once another Tenant started it (#959).

Each tool contract now states its **Tool Reach**, what the tool touches: the Run itself, the Home's Workspace, the receiving Agent version's own material, the receiving Node's Agents, people, external services, an execution environment, or never. A Reach has no default. Tools that touch only the Run itself are always usable; tools whose Reach is never are never offered (Research operations, ADR 0039; Agent Plugin tools, ADR 0100; `agent_discover` and `task_delegate`, ADR 0109; `skill_list` and `skill_load`, which serve a person's direct Skills). Every other tool is usable by a remote Run only when two lists name it:

- the Home's **Federation Tool Request**, kept on the Home's Node Registration of the receiving Node and carried by every Remote Grant in the shape of RFC 9396 `authorization_details`, and
- the receiving Node's **Federation Tool Allowance**, kept on its Node Registration of the Home.

The receiving Node also keeps a **Federation Tool Block** per peer, which wins over the Allowance. All three take tool names and `*` patterns, Registry tools as `registry:<definition>/<tool>`. A permission of the Tenant's policy, not a new role, lets a person change one Node's lists. New Node Registrations start with the tools that act only on the Home's own Workspace and people, plus the receiving Agent's Skills. Between Nodes of one Tenant there are no lists; the Tenant's policy decides as before.

Candidate tools stay in the remote Run's Binding behind deferred exposure. A call that either list lacks stops the Run in an Authority Pause and asks the side that lacks it, through the people who may change its lists. They may allow the call for this Run, add the tool, decline the call, which becomes a Declined Call, or, on the receiving side, block it. The Run waits without a time limit, and the Home renews the Remote Grant every hour while the Run lives and its delegation chain still holds. A change to either list acts as a withdrawal: the next call is refused, and commands, Live previews and Subnode members that depend on the tool stop at the next recheck.

Only the peer can look up an Allowance, through an authenticated `scoped/tools/discover`, and it sees the Allowance minus the Block. A missing, unlisted or blocked tool looks the same to the peer, blocked tools leave its Agent cards, and the receiver records every Federation token id until it expires so a replayed request is refused.

The receiving Node gives a remote Run an authority of its own, the Peer Mapping's mapped subject within the Remote Grant, and a Working Area per Home person, Agent and Home Workspace thread, kept while the Node Registration lasts. Its Reference set lives there, execution runs in the Delegated Run Scratch Space with egress limited to the hosts both sides name, files go back to the Home only, memory and `task_assign` are served by the Home, and Subnodes need `subnode_*` in the Allowance.

## Considered Options

- **Keep a fixed rule per tool, derived from its Reach alone.** Neither Tenant could widen or narrow what the other's Runs use, so a docs Tenant could not open its Reference set to one partner without opening it to all. Rejected.
- **Let the Home's request decide alone, or the receiver's allowance alone.** The Home must control where its data goes and the receiver must control its material and resources; either alone leaves one side without a say. Rejected.
- **A Node Administrator role.** It would need its own appointment and last-holder rules beside the Tenant Administrator, for what one permission already expresses. Rejected.
- **Refuse a call outside the lists without asking.** The Agent would find out only by failing, and a person would have to guess which tool to add. Rejected.
- **Offer only listed tools, and ask when the model searches for others.** The model could not try a tool it never sees, and the ask would come before anyone knew the tool was needed. Rejected.
- **Move Federation to OAuth 2.x.** The per-request signed token already binds sender, request and body; OAuth would mainly add interop with other platforms, which is #1000. Rejected here.
- **A read-only Reference view without a Working Area.** Every execution tool anchors its checks on a Working Area, so each would need a second path. Rejected once execution became listable.

## Consequences

- `remote_exposure` and the dead runtime filter are gone; the explicit-set test is replaced by one per Tool Reach.
- ADR 0109: a Remote Grant is renewed hourly and carries the Home's Federation Tool Request.
- ADR 0110: `subnode_*` in the Allowance replaces the separate receiving-Tenant setting; members use at most what the creating remote Run may use.
- ADR 0114: an Area with a Reference set may send files to the Home of its remote Run, and to no other Node.
- ADR 0118: the receiving Node's people answer asks about their lists, declining included; every other judgment stays with the Home's people.
- ADR 0126: a remote Run's memory tools are served by the Home, writing only to the Agent's participant bank.
- ADR 0064 and `docs/protocol.md`: the receiver keeps a `jti` record for Federation tokens.
- ADR 0100: Registry tools are listed like any other external service and leave the defaults.
- ADR 0111 and ADR 0136: a remote Run's commands run in its Delegated Run Scratch Space under the receiver's authority.
- Answers to an ask: "this Run only" is kept per admission on the receiving Node and per Remote Grant on the Home, where it joins the Request each Description carries; "add" and "block" are ordinary list changes with a history row; a decline of the receiving Node's own ask is a Declined Call whose sentence names no Node. `GET` and `POST .../registrations/{source_node}/tool-asks` list and answer them for holders of `node.federation_tools.manage`; the Home learns a pause only from the receiving Node's status, which names the Home's missing key and nothing about the receiver's lists.
- Receiving Working Area: the receiver binds one when a remote Run enters its worker, keyed by its Tenant, the Run's Home Node and Workspace, the Home Task (the Description carries no Home thread, so a Task is a thread here), the Run's Agent and the Home person its Delegated Run Scratch Space belongs to. The Area carries a `remote_home` constraint instead of a Workspace this Node does not hold; the Run's admission authorizes it and its owner must be on the caller's chain. It is kept as long as the Node whose Runs use it and is deleted with that Node by Node purge. A Node Registration cannot be deleted on its own, so deleting the Node is the receiver's way to delete these Areas.
- Withdrawal: a running command, a Live preview and a Subnode member's work stop at the next Read Recheck when their tool is no longer usable under the receiver's Allowance, Block and Run-only answers and the Home's Request as the admission last saw it; a Request the Home narrows since reaches them at the Run's next worker boundary.
