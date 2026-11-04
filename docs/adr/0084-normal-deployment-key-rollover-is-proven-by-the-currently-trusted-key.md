# Normal Deployment Key rollover is proven by the currently trusted key

Receivers automatically accept a successor Deployment Key only through a bounded
transition proof signed by their currently trusted key. Publish the successor,
activate its signing 24 hours later, then end verification of the predecessor
seven days after activation. One normal rollover runs at a time. During overlap,
requests signed by the successor carry the proof so admission can verify it
locally without depending on a public-identity HTTP fetch.

Apply this authentication update to Node Registrations without changing their
permissions, enabled state or authorization revisions.
Already admitted work may finish, and retries use the current key. A key update
does not justify undoing an atomic transaction's decided commit.

A receiver that misses the whole overlap needs manual confirmation rather than
indefinitely trusting a retired key to introduce successors. Lost or compromised
keys likewise require manual confirmation through an independent channel. Each
record's existing administrator controls quarantine and re-pin; replacing a key
does not re-enable a disabled registration. Independent receivers cannot
be promised instantaneous incident notification or revocation.

This extends ADR 0064's single-key model. Decided in the 2026-10-23 design rounds
for [Issue 390](https://github.com/kent8192/aidash/issues/390). See the
[completion design](../decisions/issue-244-completion.md).

Amended 2026-10-28 (#486): this ADR first applied the rollover to Peer Graph
Grants too. #486 removed the grants, so only Node Registrations hold receiving
keysets.
