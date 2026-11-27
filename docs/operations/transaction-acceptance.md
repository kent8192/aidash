# Kubernetes transaction acceptance

Run `bash scripts/test-cluster.sh kubernetes transactions` for the disposable
transaction inventory. Optional partitions are `coordinator`, `participant` and
`lifecycle`, passed as the third argument. The driver requires an explicit private
kubeconfig, exact image and Reinhardt Query diagnostic inputs, creates its own
namespace and deletes only that namespace. It never changes the current context.

The current profile supports Kubernetes with kind. Source/image identities,
case inventories and actual outcomes go under `private/transaction-acceptance/`.
A selected case or planned gate is not passing evidence. This profile does not
establish production availability, cross-version compatibility, durable-data loss
recovery, external effects, or all-six integrated release acceptance.

When a case fails, the driver also keeps the last 400 lines of every Node's
server log next to `cases.jsonl`, because a refusal's reason appears only there.

## Run of 2026-11-25 (#869, ADR 0151)

The first full run since cross-Deployment submissions resumed passed every
partition on local kind (Kubernetes v1.34.0, image `aidash:cluster-869` built
from `f18f33a334` plus the log-keeping change above):

| Partition                               | Cases | Node counts | Result    |
| --------------------------------------- | ----- | ----------- | --------- |
| `coordinator`                           | 42    | 2           | 42 passed |
| `participant`                           | 30    | 2           | 30 passed |
| `lifecycle` (scale, rolling, partition) | 30    | 2, 3, 16    | 30 passed |

Earlier attempts on the same day found and fixed one defect: a random
per-transaction seed made a resubmission after a crash at
`coordinator.submit.before` conflict with the preflight bindings participants
already held, so the seed is now derived from the manifest. One `coordinator`
case once timed out waiting 180 s for a server rollout, and one `participant`
case once received a 403 at submission; neither recurred on the full rerun.
Docker running out of disk inside the VM had caused image loss before that.

The local real-server durable-cut fixture retains each HTTP listener in its
parent across in-process handover, SIGKILL and restart. On Unix the launcher
passes that socket through `AIDASH_LISTEN_FD`, an open listening TCP descriptor
above 2 whose ownership transfers to the child. The native `server` / `serve`
launcher checks that its bound address matches `AIDASH_LISTEN`, restores
close-on-exec, and serves it through Reinhardt without rebinding. Invalid
descriptors and address mismatches fail startup; without `AIDASH_LISTEN_FD`,
the launcher binds `AIDASH_LISTEN` normally and reports bind errors with the
address. Protocol fixtures that intentionally disconnect a peer retain their
existing stop behavior. The concurrent handover regression competes for both
reserved ports throughout three child launches and native rebuilds.

The [historical September transaction register](../history/transaction-acceptance.md)
preserves exact images, revisions, commands, successes, failures and raw bundle
links from earlier distributions. Those results validate their original inputs,
not the current Kubernetes/GKE execution changes. No fresh full-matrix run is
claimed by this documentation update. See [transactions](../transactions.md) for
the recovery contract and the [release requirements](nonfunctional-release.md)
for the remaining acceptance gates.
