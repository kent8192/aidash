# The GCIP auth helper is proxied at the edge, outside admission

Aidash Cloud serves the Firebase sign-in helper from each environment's Public Origin by transparently proxying `/__/auth/*` and `/__/firebase/init.json` from Caddy to `<project>.firebaseapp.com`, and sets `authDomain` to that origin. This lets provider consent screens show the Aidash domain, and lets redirect sign-in work in browsers that block third-party storage. The route deliberately bypasses nginx's serving/draining gate and in-flight accounting. The helper grants no authority, because `/auth/gcip/exchange` is the authority boundary and it stays behind the gate. This also keeps `nginx.conf` static, while the Caddyfile is already rendered per host with the project id.

## Considered Options

- Self-hosting the helper files (Firebase "Option 4") was rejected because it does not support SAML, which tenants may use, and it requires periodic syncing of Google's helper code.
- Templating nginx to put the route behind the gate was rejected because a drained host already refuses the exchange, so gating the helper adds no fencing.
- A reverse proxy inside the Rust app was rejected because it would put upstream TLS and the Google origin inside the application.

## Consequences

- `[dashboard.gcip] auth_helper` is either `firebase` (the default, which keeps `<project>.firebaseapp.com`) or `public_origin`. Only these two values keep the helper reachable. Any other host would bring back cross-origin helper storage. Self-hosted deployments must provide their own `/__/auth` proxy before choosing `public_origin`.
- Every identity provider must register `https://<hostname>/__/auth/handler`. The SAML ACS `callback_uri` is set per environment in Terraform, while Google and OIDC registrations are manual.
- Email action links stay on `<project>.firebaseapp.com`, because the action URL is project-wide and the environments share one project.
- Because the helper is same-origin, the browser would send Aidash cookies with every helper request. The proxy strips `Cookie` and `Authorization` before forwarding to Google, and drops upstream `Set-Cookie`.
- The helper also writes `firebase:` keys (and possibly an IndexedDB store) into Aidash's storage. The sign-in page clears them after every attempt.
