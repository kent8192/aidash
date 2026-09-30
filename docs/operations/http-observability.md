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

Burst settings accept 1–100000; refill intervals accept 1–3600000 milliseconds. Invalid limits fail startup. `api::router` uses
the defaults; applications embedding Aidash can use `api::router_with_settings`.
The executable validates and loads these environment settings. The body limit is
1 MiB by default. Core capability and scoped file-transfer routes retain a 6 MiB
limit to accommodate Base64-encoded 4 MiB chunks. Each route's limit applies to
both standard extractors and streaming body reads. A streamed body reports a
size-limit error when consumed; it is not eagerly buffered.

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

`/auth/*` uses the TCP peer address supplied by
`into_make_service_with_connect_info::<SocketAddr>()`. By default it ignores all
forwarded headers. To preserve separate client budgets behind a trusted reverse
proxy, set `AIDASH_AUTH_TRUSTED_PROXY_IPS` to its exact socket IPs. Only those peers
may provide `X-Real-IP`, which must contain exactly one IPv4 or IPv6 address.
Missing, malformed or repeated headers fall back to the socket peer. `Forwarded`
and `X-Forwarded-For` remain ignored. The trusted proxy must overwrite incoming
`X-Real-IP`, and application ports must be restricted to that proxy. The GCP
Caddy/Nginx stack configures this for its loopback-only application listener;
Caddy derives identity from its client connection and Nginx replaces `X-Real-IP`.
Embedded servers
must also provide connect info; in-process tests can add `Extension(ConnectInfo(SocketAddr))`.

## Logs and request IDs

Default `RUST_LOG=aidash=info` includes one response log with the request UUID,
HTTP method, route template, status and response-start latency. The logger does
not record raw paths, queries, headers or bodies. Unmatched paths are reported
as `unmatched`. Supply a UUID in `x-request-id` to correlate an existing request;
missing or invalid values are replaced. Every response echoes `x-request-id`.
Authorization, Cookie, `x-aidash-csrf` and Set-Cookie headers are also marked
sensitive for compatible diagnostic output. This does not redact arbitrary
application logs. Existing OIDC/session validation and CSRF policy remain in
force; bearer and cookie parsing use `axum-extra`.

Private API, federation and auth route groups apply `Cache-Control: no-store` and
`Referrer-Policy: no-referrer` outside their authentication/rate-limit layers,
including authentication and permission failures. Static assets and the public
OpenAPI document do not inherit this policy. CookieJar extracts cookies once per
handler and serializes response deltas; cookie names and security attributes
remain centrally defined.

Workspace and conversation inputs share `ValidatedJson<T>`, backed by `axum-valid`
and `validator`. Blank title/goal fields return 400 with a JSON `error` containing
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

`axum-prometheus` records request counts, duration and pending requests. Endpoint
labels use route templates, with a fixed `unmatched` fallback. No user, tenant,
run, request or arbitrary path values are added as labels. Pending HTTP metrics
include the library's response-body lifecycle; use the SSE gauge to distinguish
long-lived streams from response-future concurrency.

Aidash additionally emits:

- `aidash_sse_connections`: live logical SSE connections holding admission. A body deadline or revocation releases admission even if the transport has not polled the body again.
- `aidash_sse_disconnects_total`: logical SSE admission leases released by body completion/drop or a server-initiated close. It does not confirm a physical TCP disconnect.
- `aidash_worker_active_steps`: leased steps currently handled by this process.
- `aidash_worker_steps_total{outcome="success|error"}`: completed step attempts, not terminal tasks.
- `aidash_worker_retries_total`: persisted worker retry transitions.
- `aidash_model_response_headers_seconds`: successful HTTP transport time until model response headers, not first-token latency.
- `aidash_model_tokens_total{direction="input|output"}`: reported usage from successfully parsed model responses.

The [SSE delivery runbook](sse-delivery.md) describes notification readiness, reconciliation causes, body deadlines, and query/load measurements.

These counters are operational observations, not billing records. Missing provider
usage is zero; failed/unparseable provider responses are not counted as known
usage. Durable backlog and terminal task state remain in the database and are not
represented by the active-step gauge. OpenTelemetry propagation and distributed
traces are deferred to a separate change.

## Verification

```sh
cargo test --lib http::tests
cargo test --lib cookie_tests
cargo test --lib admission_tests
cargo test --test http_protection --test dashboard_oidc --test authorization --test channel_threads --test semantic
```

Integration tests use the disposable service fixture in `tests/fixtures/compose.yaml`.

Ordinary JSON tests use `axum-test` through the shared request helpers. Non-JSON
responses fail parsing immediately, except for explicit bodyless HTTP responses.
SSE body lifetime and sensitive-header tests retain direct Tower requests.
