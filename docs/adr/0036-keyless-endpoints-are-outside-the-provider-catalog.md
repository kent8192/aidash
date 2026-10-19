# Keyless endpoints are outside the Provider Catalog

Decided on 2026-10-13 (#235). ADR 0006 says every model-provider call uses a Tenant's Provider Credential, yet #219 kept a path on which model and embedding definitions that name no `provider_credential` call their endpoint without a key. We keep that path on every Offering, for inference and embeddings alike, and call such an endpoint a Keyless endpoint. ADR 0006 protects the entry of Key Material; a Keyless endpoint involves none, so its sentence now reads: every model-provider call that carries a key uses a Tenant's Provider Credential.

A Keyless endpoint has no Provider Catalog entry. The Catalog lists only the providers Aidash calls with a grant (a Provider Credential or a Plan Connection), and Provider Authorization has nothing to authorize without a credential. A Tenant's Keyless definition is governed instead by Catalog Approval and the policy actions `registry.read` and `embedding.invoke`.

## Considered Options

- **Keyless only on Self-hosted and Dedicated.** Rejected: one rule on every Offering, with the egress restriction below covering Cloud's risk.
- **Keyless only in test builds**, as ADR 0006's wording implied. Rejected: Self-hosted Deployment Administrators would lose local models, and a model running inside the Deployment would need a key it does not have.
- **A Catalog entry for keyless providers**, redefining the Catalog as every provider Aidash may call. Rejected: it breaks ADR 0006's consequence that the Catalog admits only providers whose OAuth flow issues credentials.
- **A Deployment-hosted model as an automatic failover target.** Rejected: an index is bound to its embedding model, version and dimensions, so failover means a second index (the cost #154 rejected for an alternate provider), and Aidash never routes between models automatically (#159 also forbids fallback between grants). Outages of optional semantic context follow #154's degrade policy instead.

## Consequences

- A Tenant's Keyless endpoint must resolve to a public address, checked when the connection is made and not only when it is registered, unless the Deployment Administrators permit its private origin. This mirrors `outbound_get`. Without the rule, server and worker Pods, which can reach the metadata server and in-cluster services, would call any address a Tenant registered. Under Federation, the list of the Deployment making the call applies, because only its Deployment Administrators know what its network reaches.
- Aidash promises nothing about what a Keyless endpoint retains. The Tenant that approves the definition decides whether its data may go there, and Catalog Approval shows that no zero-data-retention guarantee applies. Zero data retention stays required on Provider Catalog paths that support it.
- A model run inside the Deployment is a Deployment-hosted model: a built-in definition whose model and version (and, for an embedding model, dimensions) Aidash fixes, so that every Deployment offers the same index contract and #122 can evaluate it. Deployment Administrators only run it and supply its endpoint, and a Deployment that does not run it offers none. Like other non-Sandboxed built-ins it needs no Catalog Approval, because what it receives never leaves the Deployment. A Tenant that wants another model registers it as its own Keyless endpoint, whose private origin the Deployment Administrators may permit.
- Aidash ships no Deployment-hosted model yet; the OpenRouter single point of failure is accepted and #154 covers embedding outages. Cloud runs none until #242 decides how its compute is billed.
- ADR 0006's sentence "Every model-provider call therefore uses a Tenant's Provider Credential" is superseded in part by this record.
