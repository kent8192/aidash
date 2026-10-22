# Stdio MCP servers of Agent Plugins run as Sandboxed sessions and install pinned packages through the Egress gateway

Decided on 2026-10-19 while grilling support for Agent Plugins 1.0.0. The format states that its path containment does not sandbox the processes a plugin starts. An Agent Plugin's stdio MCP server therefore runs as one session per Run in an Execution Pod. It is a Sandboxed capability: a Tenant enables it by approving the built-in `mcp_stdio` group, and a Deployment without a runner offers none. Its calls need no Tool call approval, because the Pod reaches nothing but the External services bound to the Run, through the Egress gateway, with a token that lasts as long as the session (ADR 0054, ADR 0070). A remote MCP server of an Agent Plugin is called from the server process instead, so its calls need Tool call approvals like a Tenant's MCP tools.

The packages a stdio server needs are pinned when the plugin is attached, as a lockfile with exact versions and integrity. A plugin that gives only `npx -y <package>` or `uvx <package>` is resolved at that moment inside an Execution Pod through the gateway, without running package code. At session start the Pod installs the lockfile from the package-index Built-in External services through the gateway (ADR 0071).

## Considered Options

- **Start the process on the server or the Node.** Plugin code would run with the server's authority. Rejected.
- **A separate Pod kind for MCP servers with its own egress allowlist.** It would be an exception to ADR 0054 that Kubernetes and Compose both need. Rejected.
- **Resolve packages on the server with npm and uv, keep the files in a Tenant store and install them offline.** It would work before the gateway exists, but it is a second installation path with different guarantees, which ADR 0071 rejected when it removed `python_install`. Rejected.
- **Install the latest packages at every session start,** as other clients do. The code a Run executes would drift from the approved version. Rejected.
- **Approve every stdio call.** Individual requests through the gateway are never approved; the approved External services bound the outside. Rejected.
- **One process per call, or one shared by all of an Agent's Runs.** The first loses server state between calls; the second breaks isolation between Runs. Rejected.

## Consequences

- Until Cloud and Dedicated have an Egress gateway (#310), their Pods cannot install packages, so stdio servers that need packages work only on Self-hosted Deployments.
- Each session start pays the installation time, because there is no package cache for now.
- A server that needs Docker or a secret in its environment does not work, unless it can address an External service through the gateway, which attaches the Tenant Secret.
- `${PLUGIN_DATA}` is kept per Working Area scope and plugin, so follow-up Runs share it in admission order and never write it at the same time.
