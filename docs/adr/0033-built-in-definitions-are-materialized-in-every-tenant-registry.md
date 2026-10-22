# Built-in definitions are materialized in every Tenant's Registry

ADR 0029 makes the Registry per Tenant while built-in definitions stay common to
a Deployment. We store a copy of every built-in definition in each Tenant's
Registry, written when the Tenant is created and re-seeded at startup, so that
every Registry identity is `(tenant, id, version)` and every reference,
foreign key and row-level policy (#223) stays within one Tenant. Built-in bytes
already embed the issuing node's identity, so once Nodes exist (ADR 0028) they
must be materialized per Node anyway.

## Considered Options

- **One shared set under a reserved tenant value.** Every reference, foreign
  key and row-level policy would need "this Tenant or the built-in set".
  Rejected.
- **Built-ins only in code, not stored.** Every lookup would have two sources,
  and the more than ten tables that reference Registry rows could not use
  foreign keys for built-ins. Rejected.

## Consequences

- Adding a built-in in an upgrade writes rows into every Tenant at startup. The
  existing byte-compare seeding keeps this idempotent.
- Built-in identifiers keep their reserved prefixes (`aidash.`, `core.`), so a
  later built-in can never collide with a Tenant's own definition.
- Built-ins remain read-only and need no Catalog Approval, except Sandboxed ones
  (ADR 0034).
