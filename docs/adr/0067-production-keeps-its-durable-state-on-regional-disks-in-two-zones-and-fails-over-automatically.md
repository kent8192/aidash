# Production keeps its durable state on regional disks in two zones and fails over automatically

Decided on 2026-10-19 in the #156 grilling. Production already has a regional GKE control plane, but its node pools and every retained disk are pinned to `us-central1-a`, so losing that zone stops the Deployment until it is restored from a backup. In production only, the six retained disks (PostgreSQL, NATS JetStream, Runner journal, Home recovery ledger, capability objects and the TLS/ACME state) move to regional pd-balanced disks, which replicate synchronously between `us-central1-a` and a second zone. The node pools, including the system pool, span both zones and autoscale; the standby zone normally runs no node. GKE's Stateful HA Operator detects an unreachable node, force-deletes its Pod and force-attaches the disk in the other zone, so every workload that holds a retained disk becomes a StatefulSet. Nonproduction Environments stay zonal under ADR 0009. Losing the whole region is accepted, and GKE backups stay in `us-central1`.

## Considered Options

- **Accept zone loss and restore from the daily backup.** Rejected: a restore loses everything after the last backup and needs manual work, while regional disks lose no acknowledged write for about USD 8.40 a month more at today's sizes.
- **A manual failover runbook.** Rejected: ADR 0009's goal is recovery without a person, and the operator gives that for StatefulSets.
- **One always-on node in each zone.** Rejected: it doubles node cost, and the goal is automatic recovery, not uninterrupted service.
- **Backups in a second region.** Not chosen: losing the region is accepted.
- **Replicated server and worker roles.** Still deferred, as in ADR 0009, until the Home recovery ledger stops depending on one filesystem.

## Consequences

- A force-attach detaches the disk from the old VM, so a Pod left running in a partitioned zone gets I/O errors instead of writing on. Storage fencing, not Aidash, keeps the ledger and the journals to one writer; operations in flight surface as uncertain, as after a node loss.
- Recovery takes the unreachable timeout, a new node in the standby zone, the gVisor installation (about five and a half minutes in the 2026-10-09 spike) and Pod start. Capacity in the standby zone during a real zone outage is not guaranteed.
- The server and the worker share the Home ledger disk, so they must move together.
- A regional pd-balanced disk is at least 10 GiB, so the chart's 36 GiB of zonal disks (USD 3.60 a month) become 60 GiB of regional disks (USD 12.00 a month). Each GiB beyond that costs twice the zonal price, and a regional disk can only grow.
- The system pool sets a total minimum of one node, not one per zone, or production pays for a second system node.
- The second zone is fixed when the disks are created, so it is chosen for N2 capacity.
- The production restore drill (release-deployment step 7) adds a zone failover drill.
