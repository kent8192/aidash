# Kubernetes transaction acceptance

Run `bash scripts/test-cluster.sh kubernetes transactions` for the disposable
transaction inventory. Optional partitions are `coordinator`, `participant` and
`lifecycle`, passed as the third argument. The driver requires an explicit private
kubeconfig, exact image and Reinhardt Query diagnostic inputs, creates its own
namespace and deletes only that namespace. It never changes the current context.

The current profile supports Kubernetes with kind. Source/image identities,
case inventories and actual outcomes go under `.ignore/transaction-acceptance/`.
A selected case or planned gate is not passing evidence. This profile does not
establish production availability, cross-version compatibility, durable-data loss
recovery, external effects, or all-six integrated release acceptance.

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
