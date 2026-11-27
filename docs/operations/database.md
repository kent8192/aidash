# Operating PostgreSQL connections

The application chart uses `database.maxConnections` to set
`AIDASH_DATABASE_CONNECTION_BUDGET` on each server and worker. Match that
value to PostgreSQL's configured `max_connections`. After reserving three
connections for PostgreSQL superusers, twenty for jobs and commands,
and eight per maximum Egress replica, the chart divides the remainder by
the sum of server and worker replicas and rounds down. Egress reserves
include the shared group and every configured IP group, each with an
autoscaler maximum of ten. Helm refuses an allocation below twelve
connections per process.

Each process uses three pools. Control receives 25% for state transitions
and locks. Work receives 60% for requests, searches, restoration, uploads
and worker jobs. Recovery receives the remaining 15%, including integer
rounding, for maintenance. Startup reports all three sizes and warns when
their deployment-wide total exceeds the live PostgreSQL setting. Local and
Compose processes use a default budget of 65, the sum of the former pool
maxima on the normal startup path.

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

The GCP Environment chart derives PostgreSQL's `max_connections` from the
container memory limit. It subtracts explicit `shared_buffers` and reserves
25% of the limit for other PostgreSQL memory and workload variation. It
divides the remaining memory by the active backend allowance and rounds
down. The default 8 GiB memory limit, 256 MiB of shared buffers and 10 MiB
per backend yield 588 connections. The backend allowance rounds up the
peak observed in the local PostgreSQL 17 benchmark with both pgvector and
PGroonga loaded. The PostgreSQL memory request is 256 MiB. Local Kubernetes
uses the same calculation from its container memory limit.
