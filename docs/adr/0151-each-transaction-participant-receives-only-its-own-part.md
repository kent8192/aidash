# Each transaction participant receives only its own part

Renumbered from 0146 on 2026-11-27: two ADRs had been given that number, and this one was added later.

Decided on 2026-11-23 (#869, #1107, #1108). Until now every participant of an atomic transaction received the complete manifest (design D14), and each participant's `transaction.disclose` permission decided which Nodes could see its targets. That permission was checked only at preflight. Preflight commits and releases its locks before the coordinator sends the manifest, so a participant could withdraw its permission and another participant would still receive its target IDs, revisions and new values. New submissions that named another Deployment had been refused with 403 since 2026-10-11 because of this gap. Instead of holding authority across Nodes until delivery, nothing of one participant's part now reaches another participant, so withdrawing a permission has nothing left to protect.

## Decisions

- **A participant receives a share, not the manifest.** A share is the transaction header (ID, coordinator, isolation, deadline), every participant's seal and only the participant's own part. The coordinator alone keeps the submitted manifest.
- **A part is sealed with a random value.** The coordinator derives one secret seed per transaction, an HMAC of the manifest under a key derived from its `core.secret_key`, and derives each part's random value from the seed and the Node address. The same manifest always gets the same seed, so a resubmission after the coordinator crashed between its preflights and its commit matches the preflight bindings the participants already hold; a seed drawn at random would not. A seal is the SHA-256 of the part, random value included. Without the random value, a short or guessable value (a revision number, a short Thread title) could be recovered from a hash by trying candidates. Only the owning participant receives its random value, and the seed never leaves the coordinator's Deployment.
- **The digest covers the header and every seal.** All participants and the coordinator agree on that digest. Each participant checks its own part against its own seal when it admits, retries and recovers. The decision a participant asks for is bound to the digest and carries no manifest.
- **Paired obligations are checked through claims.** A delegated Task completion on the home Node needs the executor's Run finalization for the same Task, and the reverse. Each seal lists one claim per Task completion and per Run finalization: the SHA-256 of `complete_task:<task>` or `finish_run:<task>`. The owner checks its own claims against its mutations; the counterpart checks that the claim it needs is listed. Task IDs are unguessable UUIDs, so a participant that does not know the Task learns only how many such mutations a part has.
- **`transaction.disclose` is removed.** A part reaches only the coordinator, which already holds what the submitter wrote, so no per-recipient check is left to make. Whether a submitter may change a target is decided by `transaction.submit` and the underlying read and write permissions.
- **The cross-Deployment pause is lifted.**
- **No compatibility.** Journals in the former shape are discarded by `federation/0031_transaction_parts`. Aidash is undeployed, and new cross-Deployment submissions were refused while that shape was current.

## Considered Options

- **Hold the disclosure permission until delivery** with a time-bound lease, answering a withdrawal with 202 while a lease is open. Rejected: it needs a distributed lease that can last as long as a transaction's deadline (up to one hour), and it adds another recovery path.
- **Treat consent at preflight as final**, so a withdrawal affects only later transactions. Rejected: it would make "withdrawn, yet still delivered for up to an hour" the specified behaviour.
- **Reveal only the paired mutation to its counterpart.** Rejected: it discloses the counterpart's Run ID and revision and needs a hash per mutation.
- **Drop the participant-side pairing check** and trust the submitter. Rejected: a manifest could then complete a Task without ending its Run.

## Consequences

- The number of Task completions and Run finalizations in each part is visible to every participant.
- `POST /federation/v0.1/transactions/{reserve,prepare,finish}` take a share, and `GET /federation/v0.1/transactions/{id}/decision` returns a decision without the manifest. A participant's records and the participant inventory show shares.
