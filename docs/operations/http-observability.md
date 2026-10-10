# HTTP protection and observability

The [v0.1.0 nonfunctional release requirements](nonfunctional-release.md) list
the availability, latency and alert decisions and evidence needed for release.
The settings and metrics below are mechanisms; their defaults are not SLOs.

The server applies process-local protection to the public router, including
static-file responses, API errors, authentication routes and federation routes.
No cluster-wide quota or automatic request retry is introduced.

## Configuration

| Variable                        | Default | Meaning                                                                            |
| ------------------------------- | ------- | ---------------------------------------------------------------------------------- |
| `AIDASH_HTTP_TIMEOUT_SECONDS`   | `30`    | Deadline until the response headers are returned (1–3600 seconds)                  |
| `AIDASH_HTTP_CONCURRENCY`       | `128`   | Shared in-flight response futures (1–65536)                                        |
| `AIDASH_SSE_CONNECTIONS`        | `128`   | Active SSE response bodies (1–65536)                                               |
| `AIDASH_AUTH_RATE_BURST`        | `30`    | `/auth/*` burst per socket peer IP; refill one request every 2 seconds             |
| `AIDASH_API_RATE_BURST`         | `120`   | Authenticated API burst per tenant/subject, or operator; refill 10 requests/second |
| `AIDASH_PEER_RATE_BURST`        | `240`   | Federation burst per authenticated node; refill 20 requests/second                 |
| `AIDASH_AUTH_RATE_PERIOD_MS`    | `2000`  | Refill interval per auth request                                                   |
| `AIDASH_AUTH_TRUSTED_PROXY_IPS` | unset   | Comma-separated exact proxy IPs allowed to provide a single `X-Real-IP`            |
| `AIDASH_API_RATE_PERIOD_MS`     | `100`   | Refill interval per API request                                                    |
| `AIDASH_PEER_RATE_PERIOD_MS`    | `50`    | Refill interval per federation request                                             |
| `AIDASH_METRICS_LISTEN`         | unset   | Optional separate Prometheus listener, for example `127.0.0.1:9090`                |

Burst settings accept 1–100000; refill intervals accept 1–3600000 milliseconds.
Invalid limits fail startup. Bootstrap injects the validated settings into the
Reinhardt Gateway shared by the public router.

The application rejects completed bodies above 1 MiB by default, or 6 MiB for
core capability and scoped file-transfer routes carrying Base64-encoded chunks.
The pinned native Reinhardt transport first collects the body under its fixed
10 MiB limit, before invoking application middleware. Application limits and
concurrency admission therefore do not yet apply during collection. Native
precollection policy support is tracked in
[reinhardt-web#6643](https://github.com/kent8192/reinhardt-web/issues/6643).
Remove this limitation only after the native listener verifies route-specific
limits for Content-Length and chunked bodies and admission cleanup on cancellation.

Global overload and SSE capacity exhaustion return **503** with `Retry-After: 1`.
Rate limiting returns **429** with `Retry-After`. An HTTP response-start timeout
returns **504**. Body-size rejection returns **413**. Request IDs and HTTP metrics
also cover these errors. Client disconnection or timeout cancels the HTTP future;
it does not undo a committed task or stop an already scheduled agent. Clients
must reconcile durable state before retrying a mutation.

API and federation rate limits run after authentication. Authentication failures
retain their existing status and do not consume another subject's budget.
Governor state is shared across router clones and routes, and stale keys are
pruned every 1024 requests. Multiple replicas have independent budgets.

`/auth/*` uses the TCP peer address supplied by Reinhardt in `Request::remote_addr`.
By default it ignores all
forwarded headers. To preserve separate client budgets behind a trusted reverse
proxy, set `AIDASH_AUTH_TRUSTED_PROXY_IPS` to its exact socket IPs. Only those peers
may provide `X-Real-IP`, which must contain exactly one IPv4 or IPv6 address.
Missing, malformed or repeated headers fall back to the socket peer. `Forwarded`
and `X-Forwarded-For` remain ignored. The trusted proxy must overwrite incoming
`X-Real-IP`, and application ports must be restricted to that proxy. The GCP
Caddy/Nginx stack configures this for its loopback-only application listener;
Caddy derives identity from its client connection and Nginx replaces `X-Real-IP`.
Embedded servers
must also provide the socket peer; in-process Reinhardt requests can set `remote_addr`.

## Logs and request IDs

Default `RUST_LOG=aidash=info` includes one response log with the request UUID,
HTTP method, route template, status and response-start latency. The logger does
not record raw paths, queries, headers or bodies. Unmatched paths are reported
as `unmatched`. Supply a UUID in `x-request-id` to correlate an existing request;
missing or invalid values are replaced. Every response echoes `x-request-id`.
Authorization, Cookie, `x-aidash-csrf` and Set-Cookie headers are also marked
sensitive for compatible diagnostic output. This does not redact arbitrary
application logs. Existing OIDC/session validation and CSRF policy remain in
force; native request adapters parse bearer and cookie credentials.

Private API, federation and auth route groups apply `Cache-Control: no-store` and
`Referrer-Policy: no-referrer` outside their authentication/rate-limit layers,
including authentication and permission failures. Static assets and the public
OpenAPI document do not inherit this policy. CookieJar extracts cookies once per
handler and serializes response deltas; cookie names and security attributes
remain centrally defined.

Workspace and conversation inputs share `ValidatedJson<T>`, backed by Reinhardt
JSON extraction and validators. Blank title/goal fields return 400 with a JSON `error` containing
sorted field names, never submitted values. JSON extraction failures retain their
400/413/415/422 status with the generic JSON error `invalid JSON request`.
Outer HTTP body-limit rejection still uses its middleware response. Storage and
authorization validation remain in place for non-HTTP callers.

Semantic search has a separate process-wide two-request concurrency layer shared
across router instances. A third search waits for a slot; this layer does not
load-shed. The outer HTTP response-start deadline still applies while waiting.

## SSE and execution

SSE admission holds a separate permit until its body completes or is dropped,
including a body that has never been polled. HTTP concurrency permits are released
when headers are returned. The response-start timeout therefore does not cut off
an established stream. Existing 15-second heartbeats, live authorization checks,
and `Last-Event-ID` replay behavior are preserved. Backpressure does not create
unbounded buffered event queues.

The existing four worker slots, durable leases, inference timeouts, cancellation,
retry rules and execution budgets remain independent of HTTP admission.

## Prometheus

Set `AIDASH_METRICS_LISTEN=127.0.0.1:9090`, then scrape
`http://127.0.0.1:9090/metrics`. This listener is not exposed on the public API
router and has no authentication. Keep it on loopback or a private network with
access limited to the collector. Configure a separate port per colocated process.
Workers can expose their own listener using the same setting.

Reinhardt Gateway retains the existing series:

- `axum_http_requests_total{method,endpoint,status}`
- `axum_http_requests_duration_seconds{method,endpoint,status}`
- `axum_http_requests_pending{method,endpoint}`

Endpoint labels come from the registered Reinhardt route metadata, with a fixed
`unmatched` fallback. Duration measures time until response headers. Pending
requests include active response bodies and release on completion, error or drop,
including bodies never polled. File responses retain their ranges and stream in
bounded chunks. Method labels use the standard HTTP methods or `UNKNOWN` for a
custom method. The `axum_` prefix is retained for dashboard compatibility; these
series are emitted by Reinhardt middleware. No user, tenant,
run, request or arbitrary path values are added as labels. Pending HTTP metrics
include the response-body lifecycle; use the SSE gauge to distinguish
long-lived streams from response-future concurrency.

Aidash additionally emits:

- `aidash_sse_connections`: live accepted SSE bodies.
- `aidash_sse_disconnects_total`: bodies completed or dropped, including normal closes.
- `aidash_worker_active_steps`: leased steps currently handled by this process.
- `aidash_worker_steps_total{outcome="success|error"}`: completed step attempts, not terminal tasks.
- `aidash_worker_retries_total`: persisted worker retry transitions.
- `aidash_model_response_headers_seconds`: successful HTTP transport time until model response headers, not first-token latency.
- `aidash_model_tokens_total{direction="input|output|input_cached_read|input_cached_write"}`: provider-reported usage from successfully parsed model responses. Cached reads and writes are breakdowns of `input`, not additional tokens.

These counters are operational observations, not billing records. A direction
receives a sample only when the provider reported that value; missing usage is
absent, not zero. Failed/unparseable provider responses are not counted as known
usage. Durable backlog and terminal task state remain in the database and are not
represented by the active-step gauge. OpenTelemetry propagation and distributed
traces are deferred to a separate change.

## Verification

```sh
cargo test -p aidash-server --lib http::observability::tests
cargo test -p aidash-server --test startup --test http_protection --test dashboard_oidc --test authorization --test channel_threads --test semantic
```

Integration tests use disposable Reinhardt Testcontainers fixtures. The `startup`
target runs the actual management binary and checks HTTP labels and sanitized
logs through its native listener. The `http_protection` target checks overload,
rate limits, authentication, SSE reconnection and body contracts; its in-process
body checks do not establish precollection transport limits. Bruno additionally
exercises every registered endpoint against actual processes.
