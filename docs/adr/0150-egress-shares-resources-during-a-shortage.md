# Egress shares resources during a shortage

Decided on 2026-11-25 in the long-running Execution and Egress grilling. Package
managers open bursts of short connections. Several Scratch Spaces installing
at once could hit a Tenant or listener cap while the replica still had
resources, and waiting sockets also held file descriptors. The gateway shared
the API process and retained a 1 MiB read buffer in each direction of every
idle Host tunnel.

## Decisions

- Remove the per-Space, Tenant, replica, listener and waiting caps. Remove the
  refusal-only slots because they served only the listener cap and no longer
  protect a resource under [ADR 0146](0146-a-limit-names-what-it-protects-counts-at-its-scope-and-says-so-when-hit.md).
- Sample each replica's open files and memory every second. Treat each
  resource as short above 80% until it falls below 70%. Calculate each Tenant's
  share as the mean among Tenants with an open connection. Count Host
  connections and External service requests together.
- Under open-file pressure alone, a connection above its Tenant's share
  waits without a fixed deadline. Credentials and Space authority still
  govern the wait. Under memory pressure, an over-share connection receives
  HTTP 503 with `egress_fair_share`, counted among pre-admission refusals.
  The gateway also releases over-share waiters already queued when memory
  pressure begins. A queued connection holds memory of its own (#995).
- Record Host wait time in `wait_ms` and show it in Run details. Report the
  replica's open connections, per-Tenant count, waiting connections, file
  descriptors, RSS, borrowed buffers and accept errors.
- Run the Helm gateway apart from the API with a database pool of eight. The
  relay borrows 64 KiB chunks from a cache while transferring data. Billing
  keeps its independent 1 MiB batches.
- Assign Egress IP groups in Deployment settings. A group can use its own GKE
  node pool and subnet, with reserved Cloud NAT addresses. Unassigned Tenants
  share the default group.

## Consequences

This amends ADR 0111 and the fixed capacity and waiting decisions in #845
(decisions 3, 13 and 17). Daily byte budgets, authority rechecks, audit
attribution, idle tunnel reclamation and the limits that govern individual
requests continue to apply. Each waiting connection still uses a client socket.
The process retries a failed accept after 100 ms on `EMFILE`. Operators should
raise the node's hard open-file limit when it is below 65,536 and verify the
public IP of each deployed group. See [Egress operations](../operations/egress.md).
