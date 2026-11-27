# OAuth Connections serve External services and HTTP tools

Decided on 2026-11-27 for #1155. This supersedes the HTTP tool restriction in ADR 0100 and extends External service authentication from ADR 0095.

An External service or HTTP tool may name a person's OAuth Connection by its canonical resource URL in `oauth_connection`. Its destination must equal that resource or lie below it. The definition chooses one credential. A required Connection that cannot be refreshed fails the call and asks the person to reconnect.

The OAuth Client may declare its provider's authorization endpoint, token endpoint, protected resource and scopes. The person chooses the registration by issuer and authorizes with PKCE. Public clients send their client ID. Confidential clients authenticate at the token endpoint with HTTP Basic. Providers can issue refresh tokens through their offline scopes.

The Run's root person owns the grant. The Egress gateway resolves that person from the Run in the authenticated process credential. The person must also authorize that Scratch Space when connecting. Its users share a Pod (ADR 0136), and the form identifies the Space before consent. Reconnecting can change or remove the Space. Each dispatch reuses ADR 0100's serialized refresh within 60 seconds of expiry. On Cloud and Dedicated the broker handles exchange, rotation and bearer attachment. The broker checks the token endpoint and protected resource sealed with each configured client secret.

The Tenant Configurator registers a client using the ID from the person's answered text Setting Request. A confidential client's secret goes straight to that registration through a secret form displaying the token endpoint host. The OAuth Setting Request then opens the browser consent flow.

For X, the OAuth Client can use `https://twitter.com/i/oauth2/authorize` and `https://api.x.com/2/oauth2/token`, with `https://api.x.com/2` as its resource. Include `offline.access` alongside the scopes the operation needs. Both an External service with base URL `https://api.x.com/2/` and an HTTP tool under that resource name `https://api.x.com/2` in `oauth_connection`. [X authorization code flow](https://docs.x.com/fundamentals/authentication/oauth-2-0/authorization-code)
