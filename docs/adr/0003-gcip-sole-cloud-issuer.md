# GCIP is Aidash Cloud's sole issuer; authorization stays in Aidash

Aidash Cloud authenticates users only through Google Cloud Identity Platform (GCIP) and does not run the direct Google OIDC configuration beside it. Aidash's policy service, mappings and Deployment Administrator grants remain the only source of authority; GCIP custom claims are never read as authority because they stay stale for the token lifetime. We chose replacement over coexistence because the dashboard model accepts exactly one issuer per deployment and treats identities from any other issuer as disabled; supporting several issuers would have been a larger change than #138, and Cloud had no production Google OIDC users to migrate.

## Consequences

- Google sign-in on Cloud goes through GCIP's Google provider. Self-hosted deployments keep the generic OIDC provider, and may also select GCIP.
- GCIP uses one `iss` for every GCIP Tenant in a project, so an External Identity is distinguished by issuer, GCIP Tenant and uid.
- Each GCIP Tenant is bound to exactly one Tenant. Identities from that GCIP Tenant can only be mapped into the bound Tenant.
- Removing a Tenant Binding disables the External Identities of that GCIP Tenant at their next boundary, as an issuer change does.
