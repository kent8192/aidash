# Aidash Account web

An independent UI for the shared Authentication Server. Build it without the
Cloud dashboard, its API generator, or Cloud runtime. The only external sign-in
adapter is GCIP; passwords and MFA verification stay there.

## Build and run

Use Node.js 24.12 or later, matching the Firebase SDK's runtime requirement.

```sh
npm ci
npm run build
AUTHENTICATION_BACKEND_URL=http://127.0.0.1:8081 npm run dev -- --port 5191
```

The development server proxies `/login/`, `/account`, `/logout`, `/oauth/` and
`/.well-known/` to the Authentication Server. Set its public issuer to the
browser-facing origin. Configure exact reverse-proxy addresses and HTTPS in
production; forwarded headers alone do not establish a trusted proxy.

The Authentication Server can serve `dist/` directly by setting
`AIDASH_AUTH_ASSETS_DIR` to the copied asset directory (for example
`/app/auth-web`). Alternatively, build the provided unprivileged Nginx image:

```sh
docker build -t aidash-auth-web .
docker run --rm -p 8080:8080 \
  -e AUTHENTICATION_BACKEND_URL=http://authentication:8081 \
  aidash-auth-web
```

Nginx proxies the owned protocol endpoints while serving `/sign-in` and
`/settings/account` as UI routes at the same origin. Its CSP permits the HTTPS
iframe used by a configured GCIP auth domain, including custom domains, and
Google's iframe API. The upstream backend URL is a runtime proxy setting and is
not bundled into JavaScript.

## Human authentication and account changes

Start `/sign-in?flow=...` through a product's registered OIDC authorization flow.
The cookie-bound `/login/context` supplies the configured directory, enabled
providers and public Firebase configuration. Users cannot submit an arbitrary
provider configuration. Google, password and configured enterprise OIDC/SAML
providers use the GCIP SDK with in-memory persistence. Password registration,
verification-email delivery and TOTP challenge/enrollment remain GCIP operations.

After proof of an unassigned Identity, users explicitly choose a new Aidash
Account or prove an existing one. Retained historical associations permit only
reattachment to the original Account. There is no email-based linking or Account
merge. The GCIP client is signed out and deleted between proof steps; email and
password fields are cleared. A transient upstream GCIP ID token goes only to
the Authentication Server's `/login/verify` endpoint. Product access and refresh
tokens are never requested, received or stored by this UI.

`/settings/account` lists an authenticated person's identity associations. Adding
or removing a sign-in method starts a server-owned fresh-proof flow with a
cookie-bound CSRF proof. The server owns all Account membership, historical
association, last-login-method and fresh-proof checks. The frontend disables
removing the last linked method as a usability aid.

The explicit all-products/all-devices sign-out calls `/logout/all` with the
Account CSRF proof and clears the protected settings view on completion. Normal
product logout remains in each product. Admitted work is not canceled by either.

Fresh GCIP authentication is checked by the Authentication Server using the
upstream `auth_time`, separately for each step. Selecting a Google account is
not a promise that Google forced its password or MFA challenge: Google documents
that it does not support forcing Google Account reauthentication. See the
[GCIP reauthentication API](https://docs.cloud.google.com/identity-platform/docs/web/reauth)
and [Google session metadata contract](https://developers.google.com/identity/siwg/security-bundle).

## Verification

```sh
npm run typecheck
npm run build
npx playwright install chromium
npm test
npm audit
```

Browser tests exercise the actual UI and its HTTP/CSRF/phase contract while
replacing the external GCIP adapter and backend responses. They do not establish
real Google/password/MFA authentication, token verification, ownership or server
authorization. The Authentication Server owns those tests; live GCIP acceptance
also needs configured provider credentials. The Node Firestore dependency pulled
by Firebase is constrained to a patched `@grpc/grpc-js` release in this package's
lockfile; the browser imports only Firebase app/auth modules.
