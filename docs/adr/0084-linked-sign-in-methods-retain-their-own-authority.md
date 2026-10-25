---
status: accepted
---

# Linked sign-in methods retain their own authority

The target Authentication Server retains GCIP as its upstream for Google, password and enterprise single sign-on, while providing the common account and downstream OAuth/OIDC issuer. It does not introduce its own password verification, MFA or enterprise identity federation. Both products use OAuth/OIDC for browser sign-in; Cloud uses internal gRPC for backend validity and revocation integration. ADR 0003's restriction to GCIP upstream remains, while the new server becomes the downstream OAuth/OIDC issuer rather than treating a GCIP token as a Marketplace-issued token.

Adding an External Identity Link requires fresh authentication of both the existing Aidash Account and the External Identity being added. An External Identity can be linked to at most one Aidash Account. If it belongs to another account, linking is rejected: the initial version does not merge existing Aidash Accounts. Matching email addresses never prove an association.

The initial version also forbids reassignment after unlinking. The first Account association is retained as history, and the Identity may be linked again only to that original Account. Removing a login Link does not clear that historical association or change existing work's Account provenance (ADR 0086). A never-assigned legacy Identity still has the initial allocation choice described in ADR 0088.

Every Product Session retains the External Identity used to authenticate. A Tenant that requires its enterprise SSO still requires authentication through that SSO, even when the same Aidash Account also has a personal Google sign-in. Linking identities neither unions their Mappings nor converts their authentication contexts into interchangeable credentials. ADR 0027's bound-pool restrictions remain authoritative for the relevant External Identity.

## Considered Options

- **Let every linked sign-in method reach every Mapping of the shared account.** Rejected: a personal sign-in would bypass enterprise sign-in requirements and the source identity's disablement.
- **Link or merge an already-owned External Identity automatically.** Rejected: transferring login authority between existing accounts also raises ownership and recovery questions that the initial linking operation does not resolve.
- **Allow an unlinked Identity to join a different Account immediately.** Rejected for the initial version: it creates an Account-transfer path without a contract for historical work, ownership or Account Suspension. Unlinking is only removal of a login method.
- **Implement password, MFA and enterprise IdP verification in the extracted server.** Deferred: retaining GCIP preserves the existing upstream authentication while the product and service boundaries change.

## Consequences

- A common account identifies the person; it does not erase upstream identity, pool, sign-in or product authority provenance.
- ADR 0086 defines link removal, administrative recovery and restoration authority, and the separate scope of upstream Identity Disablement and Aidash Account Suspension.
- This is an accepted target contract; no authentication service or linking implementation is introduced by this documentation.
