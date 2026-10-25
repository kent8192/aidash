---
status: accepted
---

# Identity links and upstream disablement have separate scopes

In the target shared authentication architecture, a person may remove an External Identity Link only after fresh authentication through another enabled Identity linked to the same Aidash Account. The last usable sign-in method cannot be removed. Sessions established through the removed Identity are revoked. Unlinking neither deletes nor transfers a product's Mappings, purchases or ownership records; it does not replace the original authority of admitted work with another Identity.

Unlinking retains the Identity's historical first Account association. The initial version permits reattachment only to that Account and prohibits attachment to a different Account, even while no active login Link exists. Existing durable work retains its original Identity and Account provenance; the original Account's Suspension remains applicable. An absent login Link alone neither stops that work nor substitutes another Identity or Account for its checks. A legacy Identity that has never been assigned to any Account remains eligible for ADR 0088's first-assignment choice.

An enterprise issuer disabling one External Identity invalidates the sessions and work derived from that Identity, while the person's other linked Identities remain available for their own permitted uses. In particular, the enterprise cannot thereby disable the person's independent Marketplace use. Aidash Account Suspension is a separate cross-product operation that blocks new protected operations through every sign-in method of that account. Product-local authority continues to be checked separately.

Only Authentication Administrators may suspend or restore a common Aidash Account. Deployment Administrators and Tenant Administrators retain their product-local authority, and the authentication role itself grants no product authority. Lifting a common suspension does not revive old sessions or automatically resume work: a fresh sign-in and an authorized manual Run resume are required where relevant.

If every linked sign-in method is unusable, the initial version relies on GCIP or the upstream IdP's own recovery. It provides no email-assertion bypass and no administrator reassignment to a newly supplied External Identity. Inaccessible ownership records are retained rather than deleted merely because sign-in is unavailable.

## Considered Options

- **Remove the last usable sign-in method.** Rejected: the normal unlink operation must not strand the account without an authenticated recovery path.
- **Treat enterprise Identity Disablement as suspension of the whole Aidash Account.** Rejected: control over an enterprise identity does not confer control over the person's independent identities, purchases or Marketplace activity.
- **Delete or move local ownership when unlinking a sign-in method.** Rejected: an authentication association is not an ownership-transfer operation.
- **Let a product administrator suspend a person's common account.** Rejected: product-local administration does not confer control over that person's independent products or identities.
- **Provide a manual replacement-Identity recovery flow in the initial version.** Deferred: it would require an independent proof and ownership-recovery contract. Upstream recovery is the supported path.

## Consequences

- Revocation and transient-outage behavior follow ADR 0085; unlinking a login method is not cancellation of admitted work.
- ADR 0088 preserves existing owner and authority references independently of the new Authentication Administrator role; initial assignment also records stable Account provenance without changing those references.
- This is an accepted target contract; link removal and account suspension are not implemented by this record.
