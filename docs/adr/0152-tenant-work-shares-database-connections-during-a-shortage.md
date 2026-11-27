# Tenant work shares database connections during a shortage

Decided on 2026-11-27 in the #1117 limits grilling (#1111). Each server or worker process opened a
main pool of 16 connections, a control pool of 16, lazy pools of 4 and 5, two recovery pools of 8
and a worker pool of 8, about 65 connections at rest. Knowledge search and federation recovery
built new pools on every call, and two searches per process were allowed at once so that those
pools would not exhaust Postgres. Postgres was fixed at `max_connections=300` with a 2 GiB memory
limit and no stated basis, the API server had no replica limit, and nothing counted connections
per Tenant: with three server and three worker replicas the Deployment already exceeded 300, and
one Tenant could hold every connection a process had. The Deployment is expected to serve more
than a hundred Tenants.

## Decisions

- **Three fixed pools per process.** `control` serves state transitions and locks, so they never
  wait behind Tenant work. `work` serves every Tenant-driven read and write, including requests,
  knowledge search, restoration, uploads and workers. `recovery` serves background maintenance.
  No code path opens a pool per call.
- **A connection budget per process, derived from the Deployment.** The budget is
  `(max_connections − 3 reserved − 20 for jobs and commands − Egress and preview gateway replicas
at their maximum × 8) / (server + worker replicas)`, split 25 % control, 60 % work and 15 %
  recovery. Helm computes it and refuses to render when a process would get fewer than 12.
- **`max_connections` follows Postgres memory.** It is
  `floor((memory limit − shared_buffers − 25 % of the memory limit) / 10 MiB)`. On 2026-11-27 an
  active backend with pgvector and PGroonga loaded used 9.3–9.7 MiB and an idle one 7.6–7.8 MiB on
  the local image (PostgreSQL 17.12, arm64), so each connection is allowed 10 MiB. The Postgres
  memory limit rises from 2 GiB to 8 GiB with `shared_buffers=256MB`, which gives 588.
- **Database fair share.** While more than 80 % of a process's work pool is in use, until it falls
  below 70 %, each Tenant holding work connections is entitled to an equal part of the pool and at
  least one. An operation over its Tenant's share waits in a per-Tenant rotation, so many Tenants
  are served in turn; after 10 seconds it is refused with HTTP 503 `database_fair_share`, naming
  the pool size and the share. Only an operation's first work connection is counted, so an
  operation that holds two connections never waits on itself. The control and recovery pools are
  not shared out by Tenant.
- **A pool that cannot hand out a connection says so.** An acquire timeout is HTTP 503
  `database_pool_busy` naming the pool and its size, not a generic database error.

## Considered Options

- **Keep the per-feature caps, such as two searches per process.** Rejected under ADR 0146: they
  counted per process while the resource is per Deployment, and they waited without a reason.
- **Raise `max_connections` to a fixed larger number.** Rejected: the number would still have no
  basis, and Postgres memory, not the setting, decides how many backends fit.
- **A connection pooler such as PgBouncer.** Deferred to #1162. Row-level security sets session
  state (`SET ROLE`, `set_config`), so transaction pooling would need changes first.
- **A fixed per-Tenant connection quota.** Rejected for the same reason as ADR 0150: it stops a
  Tenant while connections sit idle.

## Consequences

- Adding server or worker replicas lowers each process's budget; Helm fails before a process would
  fall below 12 connections, and #1162 tracks the pooler that would lift that ceiling.
- The 2 CPU cores of Postgres, not connections, bound how much work runs at once; fair share
  decides whose work waits.
- Applies the Egress fair share of ADR 0150 to database connections and follows ADR 0146.
