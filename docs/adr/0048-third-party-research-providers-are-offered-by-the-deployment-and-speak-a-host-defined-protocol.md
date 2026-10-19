# Third-party Research providers are offered by the Deployment and speak a host-defined protocol

Decided on 2026-10-15 for #227. Research providers beyond Aidash's own are declared in the Deployment's settings by whoever runs it: a transport (HTTP or MCP), an endpoint, the Research operations they implement, their Research account terms and a Deployment secret holding the key. Nobody writes host code for one. The Deployment offers it to every Tenant as a Built-in definition, like its own Research providers (ADR 0034), and each Tenant decides through Catalog Approval whether to use it. Whoever runs the Deployment is its Research account holder and pays for it. Tenants never install a Research provider or bring a search key; whether they may is #260's question, and this record does not answer it.

Every declared Research provider speaks one versioned protocol that Aidash defines, `aidash.research/v1`. Aidash builds the exact `search` or `fetch` request, as an HTTP body or as the arguments of an MCP tool, and accepts only a response in the shape of its own results. A response in any other shape is a contract failure, and the attempt stays charged. The model always sees the host's fixed tool schema (ADR 0010), whichever provider serves it.

## Considered Options

- **Tenants install Research providers from the Marketplace with their own key.** The Tenant would become the Research account holder, which decides #260's question about bringing a search key without discussing it, and Aidash would need a store for Tenant-owned secrets that ADR 0006 avoids for model providers. Deferred to #260.
- **Mapping each vendor's own API onto the contract**, for example with response templates for Tavily or by calling a vendor's MCP server directly. Aidash would no longer build the exact bytes a person approves, and vendor MCP tools return formatted text rather than structured results. Rejected: a vendor needs a small adapter that speaks the protocol.
- **The Agent transport.** It delegates a Task to an Agent, asynchronously and possibly on another Node, while Research operations stay local to the executing Node and keep their evidence there. Citing research another Run did is the cross-Run design that ADR 0039 defers. Rejected.

## Consequences

- Tenants still cannot register `research.*` definitions.
- A declared Research provider's key is named in its declaration, not in the materialized definition; no tool definition can name a Deployment secret (#296).
- The second fixture-replacement acceptance test runs against an HTTP and an MCP stub that speak the protocol.
