# GCIP Tenant Bindings are optional; self-service identities use a Shared Pool

Supersedes ADR 0003's consequence that each GCIP Tenant is bound to exactly one
Tenant, and the sign-in by Tenant name.

On Cloud, one person creates and joins several Tenants and owns a Personal
Tenant. If every GCIP Tenant were bound to one Tenant, none of this could work.
Each GCIP-based deployment therefore has one Shared Pool: a GCIP Tenant with no
Tenant Binding, offering Google and email/password sign-in. A Tenant Binding is
kept only for a Tenant that brings its own single sign-on, and a Sign-in Domain
routes that Tenant's people to its pool. Everyone else signs in through the
Shared Pool. Nobody types a Tenant name to sign in, so Tenant names can be
random (`<adjective>-<noun>-<random string>`).

## Considered Options

- **Create a GCIP Tenant per Tenant automatically.** This needs runtime IAM to
  create GCIP Tenants and per-project quota, and it conflicts with Terraform
  ownership of the pools. It is deferred to a separate design.
- **Keep sign-in by Tenant name (`?org=`).** Tenant names would have to be
  memorable, and they could then be guessed and squatted. Rejected; Sign-in
  Domains route instead.

## Consequences

- Identities from a bound pool can still be mapped only into the bound Tenant.
- A Dedicated deployment's customer pool is a Shared Pool by default; the
  customer can bind a pool to a Tenant that needs separate single sign-on.
- `GET /auth/login?org=` is removed.
