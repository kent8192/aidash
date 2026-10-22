# Read-only operations are declared by the Tenant's definition, never by MCP annotations

Decided on 2026-10-19 for #408 and #410. Every call of a Tenant's HTTP or MCP tool needed a person's approval, even one that only reads. A version of a Tenant's HTTP or MCP tool definition may now declare which of its operations are Read-only operations, and their calls need no Tool call approval. The declaration belongs to the version, so the Catalog Approval of that version covers it, and changing it needs a new version. An MCP server's annotations, such as `readOnlyHint`, describe the server in its own words, so they only prefill the registration form and never make an operation read-only.

A Read-only operation still sends its arguments to the tool's service. Its destination is a service the Tenant registered and approved, unlike a Web disclosure, whose destination comes from the conversation.

## Considered Options

- **Trust MCP annotations.** A server could exempt any of its tools from approval. Rejected.
- **No distinction.** Read-only tools would keep stopping Runs for no protection. Rejected.

## Consequences

- A Read-only operation implies no Concurrency safety.
