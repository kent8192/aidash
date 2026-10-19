# Tool call approvals join capability approvals, and every approval resumes the exact call

Decided on 2026-10-19 for #408 and #409. A call of a Tenant's HTTP or MCP tool used to raise its own kind of approval: a human request that anyone able to answer the Run's requests could approve, valid for a hard-coded 15 minutes, with no way to approve later calls. Outbound requests and Web disclosures, added later, use capability approvals instead: a Designated approver, Allow once or a Run grant, the Deployment's `approval_seconds`, and direct permission in the Tenant's policy. Both decide the same thing, whether a Run may act outside Aidash, so a tool call now raises a Tool call approval in the capability approval flow, and the old wait is removed rather than kept beside it.

Every kind of approval now resumes the exact call that was approved. Before, an outbound or Web disclosure approval returned `approval_required` to the model, which had to repeat the identical call after the approval. That cost another inference and let the model change the arguments or never repeat the call. A Tool call approval's Run grant covers one operation of the tool until `grant_seconds`, and new user input does not end it, unlike a Web disclosure's, because the tool's destination is fixed by the Tenant rather than chosen from the conversation.

## Considered Options

- **Keep the human request and add "allow for this Run" to it.** Two approval flows would keep differing in who may approve, how long approvals last and how they are revoked. Rejected.
- **Make every approval return `approval_required` to the model**, as outbound and Web disclosure approvals did. Rejected for the cost and the room it leaves the model.

## Consequences

- Only a Designated approver decides a Tool call approval. A Thread participant who could answer the Run's questions but whom the Tenant's policy does not let approve can no longer approve its tool calls.
- An expired or denied approval refuses the call; the model is told, and the Run continues.
- The evaluation escalation oracle reads the Tool call approval record instead of a denied `APPROVAL_REQUIRED` human request.
- A Run executing for another Node still asks that Node through a human request, as before, because the person who can approve is at that Node and a capability approval needs the executing Node's own authority. Approval Modes and Rules never cover such Runs. Building capability approvals at the requesting Node is left to a later Issue.
