---
status: accepted
---

# Browser sign-in uses OIDC and product backends keep tokens

In the target architecture, Cloud and the Marketplace both sign browser users in through the shared Authentication Server with OpenID Connect Authorization Code and PKCE. Cloud retains its internal gRPC integration for session-validity checks and revocation, rather than introducing a second browser-specific authentication protocol. OAuth/OIDC and gRPC use the same account, External Identity provenance and revocation state.

Each product backend acts as the confidential browser-facing client: it exchanges codes and holds downstream access and refresh tokens on the server. Product JavaScript does not receive or persist those tokens. The browser receives an opaque, HttpOnly Product Session cookie scoped to that product's own origin; cookies are not shared between products. SSO uses redirects to the Authentication Server. This concerns tokens issued by the shared server to the products; it does not redefine how GCIP's sign-in page obtains its own upstream credentials.

The initial browser Product Session and Authentication Server browser SSO session defaults are twelve hours absolute lifetime and thirty minutes idle. Refresh does not extend a Product Session's absolute lifetime. Reissuing a Product Session through SSO is distinct from fresh authentication: adding or removing an External Identity Link requires explicit reauthentication, not just an existing SSO cookie.

Downstream refresh tokens rotate in families scoped to one product client and device. Confirmed inconsistent reuse invalidates that family and its Product Session, not other products or devices. Explicit all-products logout and Account Suspension retain their broader scopes. An uncertain network response is not itself confirmed replay: refresh must retain safe response-loss retry behavior rather than treating every retry as theft or discarding a usable secret merely on transport failure.

## Considered Options

- **Exchange browser login codes through a Cloud-specific RPC protocol.** Rejected: both products can reuse the standard browser sign-in flow while internal Cloud checks continue over gRPC.
- **Let product JavaScript keep downstream access and refresh tokens.** Rejected: backend token ownership preserves the existing browser model and limits token exposure to product JavaScript.
- **Share a cookie across authentication, Cloud and Marketplace domains.** Rejected: each product maintains its own session, logout and origin scope; redirects provide shared sign-in.

## Consequences

- First-party client registration, exact callback validation and audience checks remain implementation requirements. The accepted session and refresh scope above constrain their implementation. ADR 0089 fixes internal RPC service authentication and transport; ADR 0090 preserves the Desktop broker.
- The unified Marketplace Pages project uses typed server functions within the product and the standard OIDC interface between products without importing Cloud's browser shell.
- This is an accepted target contract; the existing Cloud and Marketplace browser entrypoints are not changed by this record.

## References

- [OpenID Connect Core](https://openid.net/specs/openid-connect-core-1_0.html)
- [OAuth security best current practice](https://www.rfc-editor.org/rfc/rfc9700.html)
- [OAuth for browser-based applications, BFF architecture](https://www.rfc-editor.org/rfc/rfc10017.html#section-6.1)
