# An authority reaches the other Nodes of its Deployment, and a read it cannot confirm is not a refusal

Decided on 2026-11-06 for #751. An authority built from a `Store` had no way into this process's handlers, so asking another Node of the same Deployment always failed, and that failure read as "not visible". A Run that had only found an Agent on another Node of its Tenant hid its outputs from everyone, including a Tenant Administrator, and a Task it created could not be delegated. Tests with two Deployments missed this because they take the HTTP path, and the one test of two Nodes used a path that had been patched.

A running server now binds its in-process handlers to its `Store`, and every authority reaches the other Nodes of its Deployment by default. Only these do not ask: an authority holding the Tenant's policy row exclusively, whose peer would wait on the caller's own lock, and a process with no server, such as the memory recovery command. A check they skip never blames a Node nobody asked.

A Node that answers and refuses stays a refusal. So does a target with no route. A Node that does not answer leaves the Read dependency unconfirmed, which is a different result:

- A person reading one thing gets `503` and `node_unreachable`, with the Node's address and no content. A reader not allowed to discover that Node still gets `403`.
- A list, a search or a stream leaves the item out, and a warning log names the Node and the cause.
- An Agent's call that has not yet had its effect is retried up to 5 times, then returns to the model as a Rejected Call naming the Node. The Run goes on.
- An Agent's call that has had its effect pauses at once, because running it again could repeat the effect.
- A Read Recheck at a worker boundary retries up to 5 times, then pauses. A Run never goes on unconfirmed, so a revocation still stops it. This is not an Authority Pause, because no change of authority lifts it.

Two exclusive callers need a Read dependency check and cannot ask: stopping, deleting, approving or denying a generation request, and activating a queued one. Both keep a plain `403`, and nobody is blamed, because no Node was asked. The Home side of a foreign generation (`lease` with the lock held exclusively) first makes the check under a shared lease and then reads the Task row alone under the lock. The gap is milliseconds, and the next Read Recheck stops a Run whose reads were revoked.

The check runs in process while the caller's transaction is open, as the patched paths already did. It stops at 10 seconds, so a full connection pool ends in "cannot confirm" and not in a hang. Within one authority, a Node that did not answer is not asked again.

Delegating to a Node of the same Tenant, once the read worked, met two more refusals that no test had reached, and both are fixed here. A Run admitted without a credential could not find its Scratch Space, because the lookup went through the admission's credential and a Node of the same Tenant acting for one of its subjects holds none. The person who started the work is the root of the subject chain the admission recorded. And the Home asked the receiver to verify a usage reservation as the receiver itself, because in one database the receiver's Run row is also visible to the Home. A row that names the peer is the other end of the exchange, not the sender.

A third stop is left as it is. A Run admitted by a Remote Grant is judged at the strictest Data Protection Level, because the receipt does not publish the source Tenant's current policy. Inside one Deployment that policy is in the same database, so the assumption is stricter than it needs to be. With only Keyless model endpoints the receiver's Run stops with `Data Protection Level ZeroDataRetention: Tenant Keyless endpoints cannot meet this level`, a named reason. Reading the policy directly for a Node of the same Deployment is a separate decision.

## Considered Options

- **Fix only the delegation paths.** A person still could not read the outputs of such a Run. Rejected.
- **An `Access` constructor that takes the `Federation`, with the old one renamed.** It is explicit and needs no state on the `Store`. It touches 57 files while many PRs are open, and any site left on the old constructor keeps the silent refusal, which is this bug. Rejected.
- **Keep answering `403`.** A person looks for a policy problem that does not exist, and a Run pauses on something no authority can lift. Rejected.
- **Fail a whole list with `503`.** One Node's trouble would stop every list and stream. Rejected.
- **Close the transaction during the check and reopen it.** This rewrites every read. A dedicated small connection pool is the next step if a live check shows reads getting stuck, and it needs no API change.

## Consequences

- With #768's fair queue, a check on a Node of the same Tenant would wait behind a queued writer that waits for the outer reader. Those handlers must skip that queue.
- A list can be shorter than it should be with no sign of it until #770.
- Until #758 lands, a read whose local and peer checks together pass 30 seconds answers `504`.
- A Run that discovered an Agent on a Node later deleted stays hidden, because there is no route to that Node. Whether a listing counts as a read at all is #769.
