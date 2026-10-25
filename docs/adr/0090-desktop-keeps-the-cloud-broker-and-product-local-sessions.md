---
status: accepted
---

# Desktop keeps the Cloud broker and product-local sessions

The initial shared-authentication cutover preserves Desktop's existing Cloud broker and product-local start, authorize, exchange, refresh and revoke protocol. Browser sign-in uses the shared Authentication Server, after which Cloud issues the Desktop Product Session. Desktop does not receive a common Authentication Server refresh token. Cloud applies the common authentication validity, all-products logout and Identity lifecycle rules to Desktop-origin operations through the same boundaries as other human-origin work.

The initial Desktop defaults remain five-minute access, thirty-day idle and ninety-day absolute lifetime. Refresh does not extend the absolute limit. Existing response-loss retry semantics and successor-secret handling must survive extraction. Legacy Desktop sessions still end at cutover under ADR 0088; preserving the protocol does not preserve those old sessions.

## Considered Options

- **Replace Desktop sign-in and refresh with a direct shared-server OAuth client immediately.** Deferred: it changes an existing native-client protocol in addition to extracting authentication. Keeping the broker limits the initial migration and avoids introducing a different native token protocol at the same cutover.

## Consequences

- Desktop remains dependent on its Cloud product endpoint, not on Marketplace availability.
- A later direct native OAuth flow requires its own compatibility and migration decision.
- This records an accepted target compatibility contract; the broker implementation has not been changed.
