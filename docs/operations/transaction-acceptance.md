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

The [historical September transaction register](../history/transaction-acceptance.md)
preserves exact images, revisions, commands, successes, failures and raw bundle
links from earlier distributions. Those results validate their original inputs,
not the current Kubernetes/GKE execution changes. No fresh full-matrix run is
claimed by this documentation update. See [transactions](../transactions.md) for
the recovery contract and the [release requirements](nonfunctional-release.md)
for the remaining acceptance gates.
