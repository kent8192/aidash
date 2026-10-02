# Aidash Desktop

A Tauri 2 client for an **already running** Aidash server. It embeds the existing
`web/dist` React/Vite application; the API, agents and infrastructure stay on the
server. There is no sidecar, service installer, embedded database or native
business API.

## Run and build

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), the
repository's Rust toolchain and Node 22. From the repository root:

```sh
npm ci --prefix web
npm ci --prefix desktop
npm run dev --prefix desktop
# A packaged application, containing the same production UI as the Web edition:
npm run build --prefix desktop
```

The Vite prebuild generates the HTTP client from the backend's OpenAPI contract.
The desktop Rust crate is a separate workspace so backend and worker builds do
not depend on GUI libraries. On macOS, `-- --bundles app` builds just the app
bundle. Signing/notarization and Windows signing credentials must be supplied
by the distributor; this repository does not contain them. An unsigned local
build is not a signed production release. Automatic updates are not installed.

Add a named connection using the server's `AIDASH_OIDC_PUBLIC_ORIGIN`, including
its port. Only origin URLs are accepted: no path, credentials, query or fragment.
Remote servers require HTTPS; HTTP is permitted for `localhost`, `127.0.0.1` and
`[::1]`. The client validates TLS certificates and rejects redirects on API/auth
requests. Reverse proxies must serve the API and `/auth` at that same origin.
Multiple profiles retain separate logins; only one profile is active.

## Authentication and persistence

Configure the backend's existing Google client, client secret and registered
`/auth/callback` URI as described in the root README. No new Google client secret
is distributed with the desktop client. Servers must advertise
`desktop_protocol: 1` at `/auth/config`; an older server produces upgrade guidance.

The native process binds a temporary random `127.0.0.1` port, prepares state and
S256 PKCE, and opens the selected server's sign-in page in the system browser.
The browser completes the existing Google flow and explicitly approves the
handoff. A single-use code (60 seconds) and state return to the loopback listener.
The code is bound to the initiating browser session, server origin, callback and
PKCE verifier. Provider tokens and the Google client secret remain on the server.
There is no embedded WebView login and no custom-protocol callback registration.

The native process stores the Aidash refresh credential in macOS Keychain,
Windows Credential Manager or Linux persistent Secret Service. Namespaces include
the application, development/production build, profile UUID and normalized origin.
Linux needs an unlocked persistent Secret Service, such as GNOME Keyring; a
session-only kernel keyring and plaintext fallback are deliberately unsupported.
A locked/unavailable store reports an unlock/retry error and preserves its data.
Connection metadata alone is saved in the application configuration directory.

Short-lived access tokens cross the narrow native authentication interface and
exist only in memory. They are sent as explicit Bearer headers by `transport.ts`;
Web cookies are omitted. Refresh credentials never enter renderer storage, IPC
responses, logs or URLs. Each refresh records a proposed successor in the OS
store **before** sending it. Retrying the identical old/new pair recovers a lost
response. A used credential with a different successor, or reuse after its
successor has rotated, revokes the entire desktop session family. Requests are
serialized, and a single application instance owns the credential store.

Default backend policy, independent of Web session settings:

| Variable                          | Default   | Meaning                                |
| --------------------------------- | --------- | -------------------------------------- |
| `AIDASH_DESKTOP_ACCESS_SECONDS`   | `300`     | Access token lifetime (60–900 seconds) |
| `AIDASH_DESKTOP_IDLE_SECONDS`     | `2592000` | Session idle limit (30 days)           |
| `AIDASH_DESKTOP_ABSOLUTE_SECONDS` | `7776000` | Session absolute limit (90 days)       |

Idle and absolute limits accept 60–31536000 seconds, with idle <= absolute.
Only user activity extends the idle timestamp; background refresh/SSE traffic
does not. Absolute expiry requires another browser sign-in. Current-device
logout revokes that profile's session, while all-device logout revokes both Web
and desktop sessions for the identity. Removing a profile revokes its credential
first. Logout/removal requires the server and OS store to be available; failure
preserves the profile for retry. Google's account-level revocation limitations
are unchanged from the Web edition. Local identity, mapping, grant, policy and
session revocations remain authoritative at protected HTTP/SSE boundaries.

## Boundaries

Web cookie/CSRF authentication remains intact. Desktop CORS permits exactly
`tauri://localhost`, `http://tauri.localhost` and the fixed development origin
`http://127.0.0.1:1420`, without credentialed cookies. Deployments do not need
wildcard origins. The shared transport cancels in-flight requests and checks the
connection/authority generation through body consumption. Switching profiles
clears query caches, selection, authority and SSE cursors. Reconnection within a
single authority continues to send `Last-Event-ID`.

Only the bundled main window can invoke the seven application commands. There
are no generic HTTP, filesystem, shell, secret-read or opener IPC permissions.
Native HTTP operations target fixed `/auth` endpoints on the selected profile.
The external opener accepts only that server's validated authorization URL.
Remote navigation and new windows are denied. CSP forbids remote scripts,
frames and forms; remote API data receives no native capabilities.

## Verification

```sh
npm run build --prefix web
npm run test:graph --prefix web
npm run test:collaboration --prefix web
npm run test:ui --prefix web
npm run test:desktop --prefix web
RUSTC_WRAPPER= cargo test --manifest-path desktop/src-tauri/Cargo.toml --locked
# Uses the existing disposable Testcontainers PostgreSQL/NATS/Qdrant fixture:
RUSTC_WRAPPER= RUST_MIN_STACK=8388608 \
  AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789 \
  cargo test --locked --test desktop_auth --test dashboard_oidc
# Real packaged Tauri + WebKitGTK + Secret Service, in a disposable Linux container:
bash desktop/tests/linux.sh
```

`web/desktop-tests` checks stale JSON/headers, authority transitions, CSRF,
credential destinations, 401 refresh, uncertain mutation outcomes and SSE cursor
reconnection. The Linux native smoke uses actual IPC, HTTP, WebKitGTK, OS secret
storage and process restarts. It exercises browser handoff against a deterministic
broker fixture, not Google's live service. `desktop/artifacts/` contains its JSON
report and a Graph screenshot. The fixture reuses the Web Graph test scene.
The container disables WebKit's sandbox only for the root/Xvfb test environment;
the application does not disable it in production.

Manual release acceptance remains necessary for live Google login, locked-store
recovery, physical desktop Linux environments, signed installers, and each OS
not covered by a recorded native run. See `VALIDATION.md` for the actual local
results; CI definitions are not evidence that hosted checks have passed.

## Cross-platform native CI

The Desktop workflow runs the same WebdriverIO + `@wdio/tauri-service` acceptance
suite on standard `ubuntu-24.04`, `windows-2022` and `macos-15` runners. It uses the
[embedded WebDriver provider](https://v2.tauri.app/develop/tests/webdriver/),
including real WebKitGTK, WebView2 or WKWebView, IPC, HTTP/SSE and OS credential
storage. No paid driver subscription or external WebDriver installation is needed.

```sh
npm ci --prefix web
npm run build --prefix web
npm ci --prefix desktop
npm run build:e2e --prefix desktop
npm run test:e2e --prefix desktop
```

The runner discovers Cargo's target directory, or accepts `AIDASH_E2E_BINARY`.
Linux requires a display and unlocked Secret Service. The CI helper
`desktop/tests/wdio/linux-session.sh` sets up a disposable store under
`RUNNER_TEMP` inside `dbus-run-session -- xvfb-run -a`; it must not be used to
unlock an existing personal store. macOS CI uses a disposable test keychain.
Local runs use the real OS store with a separate `dev.aidash.desktop.e2e`
credential namespace and random profile IDs, then remove their test credentials.

The tests cover startup, unsafe-origin rejection, fixture broker handoff, Graph
canvas/labels/fit, connection changes, SSE resume/reset, cookie omission, denied
navigation/opener IPC, persistent login and logout. Three sequential WDIO
launchers start distinct native processes, verified by PID, while retaining the
same profiles and OS store. Each phase has a four-minute process timeout and no
automatic test retries. Failure cleanup stops the app and removes test credentials.
Screenshots and a JSON result are written to `desktop/artifacts/wdio/<platform>`.

Google and the OS default-browser launcher are outside this deterministic suite:
the `e2e` feature substitutes a separate Node broker client for the browser
launcher. The real PKCE exchange, temporary callback and credential adapter still
run. This is fixture authentication evidence, not live Google acceptance.

Both WDIO plugins are optional Cargo dependencies behind `e2e`. Normal builds
include neither plugin, global Tauri APIs nor WDIO permissions. The separate
`tauri.e2e.conf.json` enables those only for the instrumented debug build; a
compile-time error rejects `e2e` in release builds. Never distribute test binaries.

### Execution and storage limits

Standard hosted runner compute is free for this public repository under
[GitHub's current billing policy](https://docs.github.com/en/billing/concepts/product-billing/github-actions).
The workflow uses no larger runners and skips execution if the repository becomes
private. It runs on relevant pull-request changes or manual dispatch, without a
duplicate post-merge push run or schedule, and cancels superseded runs. Each
native job has a 30-minute timeout. Web assets are built once on Linux and shared
for one day; only small reports/screenshots/logs are retained for three days.
Native binaries and credential stores are never uploaded, and compiled Rust
targets are not cached. Only npm's download cache is reused. Artifact/cache
storage has its own account allowance; these limits are not an account spending
cap and do not change the account's billing settings.
