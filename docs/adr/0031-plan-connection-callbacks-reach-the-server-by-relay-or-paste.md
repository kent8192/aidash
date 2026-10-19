# Plan Connection callbacks reach the server by desktop relay or a pasted callback URL

Decided on 2026-10-13 (#159). Sign in with ChatGPT's plan-usage flow accepts only an HTTP loopback callback on `127.0.0.1`, where only the port may vary, so the callback reaches the machine running the browser, not a remote Self-hosted server. The server still runs the flow: it creates and keeps the `state`, the `nonce` and the PKCE verifier, and it alone exchanges the code and writes Plan Tokens to the Provider Credential Store. The callback URL (code, `state` and the issued `client_id`) reaches the server in one of two ways. The desktop client listens on a free loopback port and relays it over its authenticated session. Or a browser-only user copies the failed `127.0.0.1` URL from the address bar into the dashboard. Both deliver to the same endpoint, which accepts the URL only for the pending authorization of the same browser or desktop session.

## Considered Options

- The desktop client runs the whole flow and uploads the tokens. This is the textbook native-app flow and is closest to OpenAI's documented "sign in locally, then transfer the credentials to the VM" pattern. Rejected: long-lived, rotating refresh tokens would pass through the client. The server would need an entry point that accepts uploaded tokens, the shape ADR 0006 removed for Key Material, and would have to verify them a second time. Browser-only users would have no path.
- The server listens on `127.0.0.1` itself. Rejected: it works only when the browser and the server share a machine, needs an extra published port, and collides with tools such as Codex CLI on port 1455.
- A hosted flow with a registered HTTPS callback. Not available: OpenAI documents it as identity-only and limited to approved commercial partners. Cloud and Dedicated wait for that approval (ADR 0007, #248).

## Consequences

- ADR 0006's ban on typing or pasting covers Key Material and Plan Tokens, not authorization codes. A pasted callback URL carries a single-use code that is useless without the server-held verifier, and its `state` must match the pending authorization of the same browser and session.
- ADR 0008 holds: the desktop client never holds Plan Tokens.
- OpenAI's documentation does not describe authorizing on one machine and exchanging the code on another. The first implementation step verifies that the exchange works. If OpenAI rejects it, the fallback is the desktop-run flow above, with a server-issued `nonce` that the server checks in the uploaded ID token.
- The `ext_agent_host_id` that OpenAI requires is stable per Deployment.
