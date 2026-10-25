---
status: accepted
---

# Aidash Accounts share authentication while product authority stays local

The target architecture gives Cloud and the standalone Marketplace a common Authentication Server for human identification, sign-in, authentication sessions and token issuance. Both products use OAuth 2.0 and OpenID Connect for browser sign-in, while Cloud uses an internal gRPC interface for session validity and revocation integration (ADR 0087). Both interfaces use the same authentication rules and account records. Tenant, Mapping, group, policy and resource authorization remain owned by the product that grants them; moving authentication does not move Agent, service, worker or Provider Credentials into this human account model.

An Aidash Account identifies a person across products and has explicit External Identity Links. External Identities remain distinct by issuer and subject, including their GCIP pool. Matching email addresses never merge accounts or create links. An account or link alone grants no Tenant membership, product permission, Publisher ownership or Entitlement.

The Marketplace owns the records of a person's Library and Publisher under that person's stable Aidash Account. A Tenant's Library remains owned by that Tenant. Changing an external sign-in method does not transfer or erase those Marketplace records. Cloud Mappings and existing Billing Account ownership references are not automatically rewritten to account IDs; ADR 0088 preserves their existing Identity references during migration.

Marketplace-only sign-in creates no Cloud Personal Tenant. Cloud provisions a Personal Tenant on first Cloud use where its existing Tenancy Rules permit one. The Authentication Server initially accepts only first-party Aidash clients; supporting third-party clients requires a separate decision.

## Considered Options

- **Centralize Tenant membership and resource policies with authentication.** Rejected: it makes each product's authority depend on the other products' domain rules and prevents a Tenant removal from remaining a product-local decision.
- **Keep External Identity as the only cross-product identity.** Rejected: a product-independent person needs a stable identity while its external sign-in methods can differ. The explicit link keeps the upstream identities distinct.
- **Merge identities by email address.** Rejected: matching display or contact attributes do not establish authority to link identities.
- **Create Cloud resources on every successful shared sign-in.** Rejected: using the Marketplace would require provisioning an unused Cloud Tenant and make authentication depend on Cloud availability.
- **Accept third-party OAuth/OIDC clients immediately.** Deferred: client onboarding, consent and external compatibility commitments are outside the initial first-party scope.

## Consequences

- This records accepted ownership and client-scope decisions for a target architecture; the server extraction is not implemented.
- ADR 0084 fixes External Identity Link proof and cardinality, preserves enterprise sign-in provenance and retains GCIP upstream. ADR 0085 fixes logout scope, online checks, in-flight ordering and outage behavior. ADR 0086 fixes link removal, disablement scopes and administrative recovery. ADR 0087 fixes browser login and backend token ownership. ADR 0088 fixes legacy Identity migration; ADR 0089 fixes Reinhardt gRPC, service authentication, data ownership and the initial rollout scope. ADR 0090 preserves the Desktop broker. ADR 0091 fixes artifact acquisition and local-copy enforcement limits, and ADR 0092 fixes Recall semantics during disconnection.
- ADR 0084 applies ADR 0003's upstream GCIP restriction and ADR 0027's bound-pool restrictions to the extracted service. A shared account must not silently replace those existing contracts.
- Physical database separation is not required by this decision. Products consume authentication through its interfaces rather than directly reading or changing its records.
