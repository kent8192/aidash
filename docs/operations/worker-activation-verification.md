# Worker activation verification

The current disposable cluster profile is
`bash scripts/test-cluster.sh kubernetes`. Process checks remain
`bash scripts/test-worker-activation.sh`; workspace Rust checks are separate.
Every result must identify its exact source inputs, image digest and run record.

The [historical verification register](../history/worker-activation-verification.md)
and [machine-readable records](../history/worker-activation-results.json) preserve
the September 28/29 measurements, failed attempts, review repairs and limits.
These earlier images do not validate the new GKE/chart execution profile, and no
new latency or full activation acceptance run is claimed here. A focused passing
test does not close every scenario in the design matrix or establish hosted CI.

See [worker activation operations](worker-activation.md) and
[Kubernetes orchestration](../orchestration.md) for current deployment behavior.
