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

## Amendment accepted on 2026-10-23

Attaching a stdio server resolves its packages in a time-bounded preparation operation under the attaching person's authority. It checks the draft Agent's explicit package-index Bindings and the Tenant's Catalog Approvals. The Egress gateway issues a short-lived token confined to that preparation operation. This supplies an authority boundary before an Agent version or Run exists, while retaining the single installation path of ADR 0071.

Preparation has a separate bounded concurrency quota from Workbench tests, so importing packages cannot consume their test slots. The Deployment sets its ceilings and a Tenant may lower its own limits. Preparation uses the Runner's installation time limits and reports exhausted capacity to the attaching person.

A call sent to an MCP server whose outcome becomes unknown enters the existing Reconciliation flow. This applies even when the tool is declared Read-only: that declaration removes the Tool call approval, and the call remains Unsafe. A transport may send a request after reconnecting only when it can establish that the request was not previously sent. Reconnecting a session does not authorize replay of an uncertain call.

A legacy remote MCP session closes when its Run pauses or becomes idle and no logical tool call is open. The next use reconnects and checks the schemas of the Run's pinned tools. A legacy in-session Sampling approval or Elicitation keeps that session open within ADR 0074's waiting limits. Modern HTTP uses request-scoped protocol metadata instead of a protocol session; its acknowledged input-required waits follow ADR 0074's durable continuation policy.

Saving `${PLUGIN_DATA}` validates the recovered directory before atomically replacing its previously saved version. Cleanup after an abnormal exit may save recoverable content through the same validation. A lost Pod, invalid content, capacity violation or failed save leaves the previous saved version intact and records the failure in the Run's events. Changes that were never successfully saved are not guaranteed to survive.

All stdio servers of one Agent Plugin in a Run share one live `${PLUGIN_DATA}` area. The 64 MiB cap applies to their combined data. Pod grouping is an implementation choice that must preserve this shared view and isolation between different Runs and Agent Plugins; independent per-server copy-in/copy-out is insufficient.

At each completed logical stdio tool-call boundary, Aidash freezes the writers and publishes their Working Area changes, so the Run's next tool can read those files. A modern intermediate `InputRequiredResult` is a continuation checkpoint, not a completed logical call or a Working Area publication: during human waiting the Pod stays frozen and its writer ownership is retained. If another tool has advanced the Working Area revision, affected stdio sessions restart against that revision before their next call, carrying forward saved Agent Plugin data and reporting the reset. Working Area publication and `${PLUGIN_DATA}` persistence remain separate boundaries.

Attachment-time dependency pinning targets the supported Linux amd64 and arm64 Runner environments and records CPU and Node/Python compatibility information with the fixed dependencies. An incompatible receiving environment skips the affected stdio server and reports the reason. Resolving dependencies again requires a new Agent version; Run start never silently replaces the approved lockfiles.

A remote server's Tenant Secret binding explicitly selects raw or Bearer header form; Authorization defaults to Bearer and other header names to raw. That binding replaces a package header with the same case-insensitive name, while transport-owned header names remain invalid. Catalog Approval shows the effective header name and form, without revealing the Secret value. The origin restriction and current-Secret checks continue to apply.
