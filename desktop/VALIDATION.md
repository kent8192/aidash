# Desktop validation — 2026-10-02

Implementation checkout: `feat/tauri-desktop`, based on
`d1201622a4110a5d4fb908a15025752f9cae10d2` (`develop/0.1.0`, including PR #100).
These are local results for the working tree, not hosted CI or release evidence.

## Browser policy review fixes — 2026-10-03

The production consent page now permits only its validated callback origin and
port in `form-action`. Its document referrer policy retains Origin on the
same-origin consent POST while withholding referrers from the callback. HTTP
IPv6 literal connection profiles are rejected with supported alternatives, and
the invalid IPv6 CSP host-source has been removed.

Local regression results on macOS arm64 with Chromium 153.0.8010.12:

- The production Axum consent page was rendered and submitted in Chromium for
  two different ephemeral callback ports, then both PKCE exchanges succeeded.
  Another loopback port remained blocked, and callbacks received no referrer.
  Run `bash scripts/test-desktop-browser.sh`; the Desktop CI workflow runs this
  explicitly rather than relying on the Node broker acceptance fixture.
- All 8 desktop renderer/transport tests passed with the configured production
  CSP. Actual `/auth/config` requests succeeded through `127.0.0.1` and through
  `localhost` backed separately by IPv4 and IPv6-only listeners.
- All 17 native unit tests passed, including HTTP IPv6 rejection and guidance.
- Both desktop broker integration tests and all 4 existing OIDC integration
  tests passed against disposable PostgreSQL/NATS/Qdrant services.
- Backend/native Clippy passed with warnings denied. Rust formatting, ESLint,
  Prettier, shell syntax and whitespace checks passed for the changed sources.

These checks do not establish live Google login, a new packaged application run,
or hosted CI results for the review-fix commits.

## Original client validation — 2026-10-02

The cross-platform CI follow-up also passed the WebdriverIO suite locally on
macOS arm64: six checks across three distinct native processes, including real
Keychain persistence, connection switching, SSE cursor recovery, Graph rendering,
permission denial and logout persistence. The dedicated test build uses a
separate broker client process in place of the default-browser launcher; it does
not establish live Google or OS browser-handler acceptance. Normal Cargo builds
were checked to exclude both optional WDIO plugins. Windows and the new Linux
WDIO suite are exercised by the PR's three-OS Desktop workflow; consult those
checks for hosted results rather than treating this local record as CI evidence.

## Verified

| Check                                                  | Result                                                                   |
| ------------------------------------------------------ | ------------------------------------------------------------------------ |
| Shared production Web build                            | Passed (existing large-chunk warning remains)                            |
| Web UI regression suite                                | 169 passed                                                               |
| Graph and collaboration unit suites                    | 69 + 37 passed                                                           |
| Desktop transport browser suite                        | 5 passed                                                                 |
| Existing OIDC + desktop broker integration             | 4 + 2 passed against disposable PostgreSQL/NATS/Qdrant                   |
| Authentication library tests                           | 9 passed                                                                 |
| SSE delivery integration                               | 19 passed; the existing long-running benchmark was intentionally ignored |
| Native Rust tests on macOS                             | 10 passed                                                                |
| Backend and native Clippy                              | Passed with warnings denied                                              |
| Targeted Trunk check                                   | 13 files, no issues                                                      |
| ESLint, Prettier, Ruff, Rust formatting and whitespace | Passed for the changed sources                                           |
| macOS debug `.app` packaging                           | Passed; shared production UI embedded                                    |
| macOS native startup                                   | Connection screen rendered; HTTP remote origin was rejected              |
| Linux native smoke                                     | Passed, including logout persistence; details below                      |

macOS: 26.6.2, arm64. Linux: Debian Bookworm container on arm64, Xvfb,
WebKitGTK 2.50.6 and GNOME Keyring 42.1. Tauri Rust/API/CLI 2.12.1; native
credential adapter keyring 3.6.3. The Rust lockfile is included with the sources.

The Linux smoke runs the actual Tauri binary with embedded production assets.
Its API/broker fixtures reuse the Web Graph scene. It confirmed:

- System-browser opener → temporary loopback callback → native credential storage.
- Persistent Secret Service login restored after exiting and restarting the app.
- Two independent connection profiles with preserved login and reset authority.
- SSE reconnect sent the last cursor; switching servers restarted at `-1`.
- Graph View canvas, labels and fit control rendered under WebKitGTK.
- Remote WebView navigation was denied and generic opener IPC was rejected.
- Desktop requests sent no browser cookies.
- Current-device logout revoked the fixture session and remained logged out after restart.

The WebKitGTK run revealed unreadable native light select controls in the dark
Graph View. Shared CSS now supplies their appearance, and the native screenshot
was checked after the fix. The desktop frame also reserves height for its
connection toolbar without extending the shared dashboard below the window.

Evidence is generated in the ignored `desktop/artifacts/` directory:
`native-results.json`, `linux-webkitgtk-graph.png`, `driver.log`, and the local
macOS `Aidash.app`. The reproducible commands are in [README.md](README.md).

The following screenshot is retained from the Linux native run with synthetic
fixture data. The fixture deliberately reconnects SSE streams to test cursor
recovery, which can show the reconnecting indicator.

![Shared Graph View in the Linux Tauri client](docs/linux-webkitgtk-graph.png)

## Not verified / release work

- Live Google login against the user's deployed OAuth configuration. The native
  test uses an external-browser broker fixture; real broker PKCE, consent,
  single-use exchange, refresh replay/recovery and revocation use backend tests.
- Local Windows runtime behavior, including Windows Credential Manager and
  WebView2; these require the hosted Windows job or a separate Windows run.
- Physical Linux desktop environments, a locked/unavailable OS store during
  sign-in, screen readers, and signed production installers.
- Release signing/notarization, installer distribution and automatic updating.

The built app is a development artifact. Sidecars, background services and bundled
PostgreSQL/NATS/Qdrant remain outside this client. The default desktop CSP blocks
external font stylesheets; installed fallback fonts are used in the native
screenshot. API data does not gain native permissions.
