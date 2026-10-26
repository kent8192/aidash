# An Agent Plugin is a pinned copy inside the Agent version and adds no Registry definitions

Decided on 2026-10-19 while grilling support for Agent Plugins 1.0.0. An Agent takes in an Agent Plugin, not its Tenant: the Agent version holds a copy of the whole plugin directory, pinned to its commit and digest, together with the lockfiles of its stdio MCP servers. Only that Agent's Runs use what the copy carries, and changing it needs a new Agent version and therefore a new Catalog Approval. Its Skills behave as direct Skill attachments (#51), and an Agent with Agent Plugins runs only on its home Node.

## Considered Options

- **Import each Skill and MCP tool as its own Registry definition**, each with its own Catalog Approval. One package would be scattered across many definitions, a bundle cannot hold Skills, and the plugin would belong to the Tenant rather than to the Agent that uses it. Rejected.
- **Represent the imported plugin as a Shared Package definition.** A Shared Package is one definition that an owner Tenant publishes from an approved registered source, with permissions. An external repository has no owner Tenant, and the format declares no permissions. Rejected; a Tenant may still publish an Agent version that carries Agent Plugins.
- **Keep only a reference and fetch the plugin at Run start.** The content a Run executes could differ from the content that was approved. Rejected.

## Consequences

- The approval screen of an Agent version lists every MCP server of its Agent Plugins, and every component that was skipped.
- Two Agents that need the same plugin each carry their own copy.
- A Shared Package publication carries the copy and shows its origin and license. The receiving Tenant approves the Agent version itself.

Terminology amended on 2026-10-23: the existing Tenant-to-Tenant distribution is a Shared Package. The separately designed central Marketplace can be another source of Agent Plugins; its purchase and Entitlement work does not expand Issue #414's distribution scope. In either source, the executing Agent version holds its own pinned copy.
