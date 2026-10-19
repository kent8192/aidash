# MCP sampling and elicitation wait inside the open tool call, and a sampling completion is an Inference Attempt of the Run

Decided on 2026-10-19 while grilling support for Agent Plugins 1.0.0. An MCP server sends a sampling or elicitation request while one of its tool calls is still open, and it waits for the answer on the same session. Aidash's other approvals come before an action, so a Run can release its lease and wait for them durably. These cannot: the Run keeps its lease and the server's session, and waits no longer than the Deployment's approval expiry. On expiry, Aidash refuses or declines to the server, and the tool call goes on with whatever the server returns.

A Sampling approval is addressed to a Designated approver and may be given once or as a Run grant for that server. Approval Modes apply: Don't ask refuses, Bypass approves within the Tenant's policy, and Auto asks a person. The completion is written by the Run's own model through its Provider Credential Binding, as an Inference Attempt of the Run, without any of the Run's context, without tools and with a capped `maxTokens`. An Elicitation is answered by the people of the Run's Thread, like the model's question, within the shorter of their Answer Deadline and the approval expiry.

## Considered Options

- **Wait durably and repeat the tool call on resume.** The server's session would be gone, and an `Unsafe` call could run twice. Rejected.
- **Refuse sampling altogether.** Rejected by the maintainer; sampling is supported with its own approval.
- **Let the Agent version's Catalog Approval admit sampling up to a budget.** Nobody would decide individual requests. Rejected; Run grants and Approval Rules keep the load low instead.
- **Keep sampling cost out of Usage Records,** as Decision Attempts do. The cost would escape the Run's budget. Rejected.
- **Approve the request and then the completion.** The completion carries none of the Run's context, and two waits rarely fit within the expiry. Rejected.

## Consequences

- A worker holds its lease while a person decides, so approval notifications must reach approvers within the expiry.
- Auto never approves sampling by itself until a Decider can answer a question about it.
- Workbench tests deny sampling and elicitation, as they deny other approvals.
