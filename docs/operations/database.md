# Operating PostgreSQL connections

The application chart derives PostgreSQL's connection capacity from
`database.postgresMemoryLimit`, `database.sharedBuffersMiB` and
`database.activeBackendMiB`. Match these to the database deployment. The
chart reserves three connections for PostgreSQL superusers, twenty for
jobs and commands, and eight per maximum standalone Egress and Preview
replica. Egress reserves cover the shared group and every configured IP
group, each with an autoscaler maximum of ten. Standalone Preview runs
one replica. Its database work shares the replica's pool of eight.

The chart divides the remaining connections by the sum of server and
worker replicas, rounds down, and sets `AIDASH_DATABASE_CONNECTION_BUDGET`
on each server and worker. Helm refuses an allocation below twelve
connections per process.

Each process uses three pools. Control receives 25% for state transitions
and locks. Work receives 60% for requests, searches, restoration, uploads
and worker jobs. Recovery receives the remaining 15%, including integer
rounding, for maintenance. Startup reports all three sizes and warns when
their deployment-wide total exceeds the live PostgreSQL setting. Local and
Compose processes use a default budget of 65, the sum of the former pool
maxima on the normal startup path.

Atomic transaction submissions use Work for the authority transaction they
hold while waiting for peer preflight. Federation protocol route lookup and
inbound admission use Control so peer preflight can proceed when Work is
full.

## Database fair share

Each Work pool enters pressure above 80% usage and clears it below 70%.
It divides its size by the number of Tenants with active Work borrows,
rounds down, and gives each Tenant a share of at least one operation.
During pressure, the first borrow of an operation waits if it would take
the Tenant over its share. The queue serves eligible Tenants round-robin
and returns HTTP 503 `database_fair_share` after ten seconds if the Tenant
is still over share. The response names the pool size and the current share.

Later borrows in the same operation still use Work pool connections.
They bypass the admission queue so an authority lease can coexist with a
worker step transaction or a serial knowledge search read. Direct SQLx
access to the Work pool also bypasses the gate and is bounded by the
driver's pool size and acquisition timeout.

System work uses the same Work slots and contributes to pool usage. It
queues as one lane and is excluded from the Tenant share calculation.
Control and Recovery have no per-Tenant admission. An acquisition timeout
returns HTTP 503 `database_pool_busy` with the pool name and its size.

## PostgreSQL memory sizing

The application and GCP Environment charts derive PostgreSQL's
`max_connections` from the container memory limit. They subtract
`shared_buffers` and reserve 25% of the limit for other PostgreSQL memory
and workload variation. They divide the remaining memory by the active
backend allowance and round down. The default 8 GiB memory limit, 256 MiB of shared buffers and 10 MiB
per backend yield 588 connections. The backend allowance rounds up the
peak observed in the local PostgreSQL 17 benchmark with both pgvector and
PGroonga loaded. The PostgreSQL memory request is 256 MiB. Local Kubernetes
uses the same calculation from its container memory limit.
