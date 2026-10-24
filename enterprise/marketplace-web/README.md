# Aidash Marketplace web

An independent Rust application for the Agent Plugin Marketplace, rendered in
the browser by Reinhardt Pages and WebAssembly. It does not
import the Cloud dashboard, run Cloud API generation, or require Cloud to build.
The Night Ops colors and self-hosted IBM Plex faces match the existing Aidash
design language. Existing Shared Package screens remain in the dashboard.

## Local development

Use Rust 1.96 or later, Node.js 22.12 or later, and wasm-pack. Run from this directory:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --version 0.15.0 --locked
npm ci
MARKETPLACE_BACKEND_URL=http://127.0.0.1:8082 npm run dev -- --port 5192
```

Vite proxies `/api/marketplace/` and `/auth/` to the standalone Marketplace
backend. Register the backend's OIDC callback at the browser-facing origin,
for example `http://127.0.0.1:5192/auth/callback`, using the backend's explicit
development-cookie configuration for local HTTP. Production requires HTTPS.
The frontend has no OAuth client secret, access token, refresh token or Firebase
configuration. The backend holds those credentials and issues an HttpOnly
session cookie. Writes send the separate CSRF proof returned by `/session`.

The catalog and version details are public. Library, Publisher registration,
listing creation and immutable version publication use actual backend APIs.
Free acquisition is available through the backend's entitlement endpoint. Paid
prices are visible; payment checkout is not offered until a Billing adapter
exists. Library downloads use the backend's protected download endpoint.

The Reinhardt `ClientLauncher` owns the application's reactive scope. The
`app`, `publishers`, and `ui` Rust modules create Pages components, typed API
resources, forms, and reactive state. `src/main.ts` only initializes the WASM
module and imports the stylesheet; there is no JavaScript UI framework. Hash
routes retain static hosting and existing Marketplace links. BFF routes and
downloads stay outside the client router. After changing Rust during local
development, run `npm run build:wasm:dev` to refresh the WASM module.

## Plugin bundles

Publisher tools accept a JSON object mapping ASCII relative paths to base64
contents, including the root `plugin.json` and the plugin's files. The manifest
uses the Agent Plugins schema (`$schema` is
`https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`). For
example:

```json
{
  "plugin.json": "eyIkc2NoZW1hIjoiaHR0cHM6Ly9hZ2VudC1wbHVnaW5zLm9yZy9zY2hlbWFzLzEuMC4wL3BsdWdpbi5zY2hlbWEuanNvbiIsIm5hbWUiOiJzb3VyY2UtcmVhZGVyIn0=",
  "skills/reader/SKILL.md": "UmVhZCBzb3VyY2VzLg=="
}
```

The backend validates the manifest, paths, contents and immutable version, then
returns the canonical Agent Plugin tree digest. A hash of the JSON transport
is not the Agent Plugin digest. The UI displays the backend's returned digest.
The backend remains responsible for authoritative validation and permissions.

## Build and deployment

```sh
npm run build
docker build -t aidash-marketplace-web .
docker run --rm -p 8080:8080 \
  -e MARKETPLACE_BACKEND_URL=http://marketplace:8082 \
  aidash-marketplace-web
```

`dist/` contains only static assets. The provided unprivileged Nginx image serves
them and proxies the two backend route prefixes at the same browser origin.
Set `MARKETPLACE_BACKEND_URL` to a backend reachable from the container and
terminate HTTPS at the deployment ingress. The browser-facing callback origin
and backend's configured product origin must agree. Nginx does not share cookies
with Cloud or expose backend tokens to JavaScript. Its runtime environment
variable selects an upstream; it is not bundled into JavaScript.
The Docker build compiles the independent Rust crate before bundling assets.
The content security policy allows same-origin WASM compilation through
`wasm-unsafe-eval` without allowing JavaScript string evaluation.

## Verification

```sh
npm run typecheck
npm run build
npx playwright install chromium
npm test
```

`typecheck` checks the Rust browser target and the TypeScript build/test tools.
`build` runs wasm-pack and Vite; Playwright serves the built output through
Vite preview. The committed independent Cargo lockfile keeps this application
build separate from the Cloud and Enterprise server workspaces.

Playwright exercises the built WASM UI's HTTP contract with controlled backend
responses: public provenance/permission metadata, BFF sign-in links, CSRF-bound
free acquisition, Library download links, fail-closed protected views, Publisher
registration/publication, errors, empty states and keyboard navigation. These
tests do not establish the backend's authentication or entitlement enforcement;
the Marketplace backend owns that verification.
