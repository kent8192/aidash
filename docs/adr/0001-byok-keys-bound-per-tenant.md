# BYOK keys are bound per Tenant, not referenced from the Registry

Registry entries are node-global, written only by Deployment Administrators and shared with Tenants through Catalog Approval. A Registry model or embedding configuration therefore only declares that it needs a tenant key (`provider_credential: "<provider-catalog-id>"`). Each Tenant supplies the key through a Provider Credential Binding, keyed by Tenant and Provider Catalog entry. Credential IDs never appear in Registry metadata, Marketplace packages or imports. As a result, no configuration can name another Tenant's Provider Credential, and one shared model definition serves every Tenant with that Tenant's own key.

## Considered Options

- Store `credential:<uuid>` in the Registry configuration (the original #136 proposal). Rejected because each Tenant would need its own copy of every model entry. A Catalog Approval mistake would also let one Tenant's Runs spend another Tenant's key, and preventing it would rely on checks at approval and at resolution rather than on the data model.
- Let the user choose a key when a Run starts. Rejected because it moves a security binding into every Run request and complicates federated and background execution.

## Consequences

- A Tenant that approves a BYOK model but has no binding fails at Run admission with an explicit error. There is never a fallback to a key in the server's environment.
- A Run pins the Provider Credential ID chosen at admission (#106). Each call resolves the credential's currently pinned secret version, so a rotation takes effect on the next call, and a revocation fails the next call.
- On a federated receiving node, resolution uses the binding of the local Tenant that the peer mapping targets. Federation messages carry no credential data.
