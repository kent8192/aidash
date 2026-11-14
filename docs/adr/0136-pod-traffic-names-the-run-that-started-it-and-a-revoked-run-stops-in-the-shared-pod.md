# Pod traffic names the Run that started it, and a revoked Run stops in the shared Pod

Decided on 2026-11-14 (#745, with #729). Agent tools move into the Execution Pod of their Scratch Space, which every Agent using that Scratch Space shares. Until now the Pod carried one proxy credential, a MAC over the Tenant and the Scratch Space, so a Host connection could not say which Run made it, and the Read Recheck of ADR 0116 could not stop a revoked Run's traffic there. ADR 0070 promises that a Live preview stops when its Run's authority is revoked.

Each process an Agent starts in the Pod now carries a credential for its Run: a MAC over the Tenant, the Scratch Space and the Run. The server passes it with each command, interpreter session, stdio MCP session and Live preview, and the Runner sets `HTTPS_PROXY` for that process alone. The Pod's own environment carries no credential, so code that no Run started reaches nothing. A Host connection record names the Run. When the Read Recheck refuses a Run, it leaves a durable mark on that Run, and Host connection admission refuses a marked Run's credential without walking its reads again. The same refusal stops the Run's foreground commands, Live previews, Python session, stdio MCP sessions and Background processes. Files the Run wrote stay in the Scratch Space.

Code in the Pod runs as root and can read another process's environment, so it can use another Run's credential. The Run on a record is therefore attribution and a way to stop work, not a security boundary. The boundary stays the Scratch Space: its members, its allowed hosts and its External services.

## Considered Options

- **Stop nothing that is already running; refuse only new tool calls.** Results of a revoked read would keep leaving through running processes, contrary to ADR 0070. Rejected.
- **A credential per command.** It names the same Run and adds nothing a Run credential lacks, while Background processes and previews outlive their command. Rejected.
- **Recheck a Run's reads at every Host connection.** This is the per-request walk that #752 measured growing with a Workspace. Rejected.
- **Isolate Runs from each other inside the Pod.** It contradicts the shared drive of ADR 0111 and would need a user or namespace per Run under gVisor. Rejected.

## Consequences

- A revocation reaches Host connections within the Read Recheck period plus the time a resolved credential is reused (#845).
- A preparation Pod that resolves a stdio MCP server's packages (ADR 0073) carries a credential for its preparation operation instead of a Run.
- A Run delegated to another Node runs in that Node's Scratch Space under a Run of that Node, so its credential and mark are local there. A Live preview whose Run has no local grant keeps the rule of ADR 0116: unconfirmed checks stop it after the limit.
- Usage stays charged per Scratch Space (ADR 0111); the Run on a record is not a billing split.
- Amends ADR 0111 and ADR 0070.
