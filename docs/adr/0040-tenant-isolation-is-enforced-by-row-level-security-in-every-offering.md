# Tenant isolation is enforced by PostgreSQL row-level security in every Offering

Decided on 2026-10-13 (#223). Tenant isolation rested only on application code:
every query filtered by Tenant, the server logged in as the PostgreSQL superuser
that owns every table, and nothing in the database stopped a query that forgot
its filter. On Cloud such a query discloses another customer's data.

Every Tenant-owned table now carries a `tenant` column with a row-level policy,
enabled and forced, in every Offering. The server opens each transaction with a
typed scope: one Tenant; Deployment Scope with a declared reason; or no Tenant,
for work that touches only Deployment-wide data such as signing in (amended
2026-10-15, #263). The policy admits only the scope's Tenant's rows, admits every
row in Deployment Scope, and raises an error in the no-Tenant scope or when no
scope is set. Application Tenant filters stay. The policy
is a second barrier against application bugs; it does not defend against SQL
injection or a compromised server, either of which can set the scope itself.

## Considered Options

- **Enforce only on Cloud, or make enforcement a setting.** Two code paths to
  test, and missing-scope bugs would surface only on Cloud. Rejected.
- **Carry the scope implicitly in a task-local value read by a custom backend.**
  Spawned tasks lose it, and the type system cannot see a wrong scope. Rejected.
- **Set the Tenant per session when a connection is checked out.** The pool
  leaks it to the next borrower. Rejected.
- **Deployment Scope as `SET LOCAL ROLE` to a `BYPASSRLS` role.** Since
  PostgreSQL 16 only a role holding `BYPASSRLS` can grant it, and managed
  services such as Azure Database for PostgreSQL (15 and earlier) and RDS do not
  give their administrators that attribute, so Self-hosted Deployments there
  could not run. Rejected for a policy branch on a transaction-local setting.
- **Policies that reach the Tenant through parent tables (`EXISTS`).** Some
  chains are several tables deep and sit on hot paths such as event replay.
  Rejected: the Tenant is copied onto every Tenant-owned table instead.
- **Wait for Reinhardt to gain policy operations.** Rejected: policy DDL is a
  documented `RunSQL` exception until Reinhardt can express it.

## Consequences

- Two roles: an owner that only migrations use, and the server's login, which is
  `NOSUPERUSER NOBYPASSRLS` with DML only. The server refuses to start if its
  login is a superuser, has `BYPASSRLS` or owns a table. Connections are reset
  when returned to the pool.
- `FORCE ROW LEVEL SECURITY` binds the owner too, so seeding and migration-time
  data writes open a scope like the server does.
- A read that matches no row never evaluates the policy, so the database's error
  is a backstop. The guarantee is that no transaction can be opened without a
  scope; autocommit statements and the ORM's global connection are removed.
- Every table is classified Tenant-owned or Deployment-wide in one checked-in
  list per Cargo workspace (Community and Enterprise). A test compares the list
  with the database's policies and fails on any unclassified or mismatched
  table, so later phases (#243's Registry, #241's Nodes and peers) must
  reclassify their tables.
- Child tables keep their `tenant` equal to their parent's through composite
  foreign keys. Every `tenant` column references the Tenant's root record, which
  survives Tenant Deletion's purge as a tombstone so retained audit history stays
  valid; Tenant names are never reused. The policy bundle stops being that root.
- Rows without a Tenant in a Tenant-owned table, such as a Deployment Administrator's decision
  on an unaddressed Registration Request, are visible only in Deployment Scope.
- Tests run as the server's role, and the activity observer opens Deployment
  Scope itself rather than holding `BYPASSRLS`.
- Amended 2026-10-15 (#263):
  - The scope is fixed when the transaction begins, and savepoints inherit it.
    Every application port's `begin` takes it, so a use case, not an adapter,
    chooses the Tenant.
  - It reaches the database as `aidash.scope` (`tenant`, `deployment` or
    `none`), as `aidash.tenant` in a Tenant scope, and as
    `application_name = deployment:<reason>`. The policy functions and the
    activity observer share this string contract, which is documented beside
    the policy migrations. Only the code that opens transactions sets these
    settings.
  - Each Deployment Scope reason declares whether it writes; a read-only reason
    opens a `READ ONLY` transaction. A Deployment Administrator writes rows
    without a Tenant into Tenant-owned tables through a read-write reason.
  - The classification list marks a Tenant-owned table pending until its policy
    is enforced; #264 and #265 move tables out of pending.
  - Authentication never needs Deployment Scope, because credentials name their
    Tenant (ADR 0047).
- Amended 2026-10-17 (#264):
  - SQL does not promise that `OR` short-circuits, so the Tenant function
    returns NULL in Deployment Scope instead of raising. It raises when a Tenant
    scope carries an empty Tenant name, so such a scope cannot see the rows
    without a Tenant.
  - A policy error means a server bug. It is reported as an internal error, not
    as a refused permission, and it never names a Tenant.
  - A policy raises only when a row is scanned. A migration that changes
    Tenant-owned rows after the table's policy exists would pass on an empty
    database and fail on a Deployment with data, so no such migration is
    allowed. Seeding happens in the server, inside a scope.
  - The foreign keys from each `tenant` column to the Tenant root record land
    with #265, for the tables that already had a `tenant` column too.
