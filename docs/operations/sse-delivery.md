# UI event delivery

Each API process subscribes independently to `aidash.<node-suffix>.events` using
Core NATS. The existing PostgreSQL Outbox publisher and JetStream `execution`
consumer keep their roles. An incoming CloudEvent only wakes local readers:
`GET /api/events/stream` reads the canonical PostgreSQL log under current scope,
resource authorization and the Node transaction visibility gate. It never sends
broker-supplied data to a browser or fetches a supplied `dataref` URL.

The SSE `mesh` event, CloudEvent body, numeric ID, `Last-Event-ID` precedence,
negative tail cursor and adjacent JSON replay endpoint are unchanged. Retained
generations close the read/wait race. Each connection drains at most 100 raw
candidates per page and continues immediately across denied or full pages.
Workspace notifications wake matching filtered streams and global streams;
Node-wide notifications and reconnection wake all local streams. Registrations
exist only while readers are active.

## Settings and lifecycle

| Environment variable | Default | Allowed values |
| --- | --- | --- |
| `AIDASH_SSE_RECONCILE_INTERVAL_MS` | 5000 | 250–60000 |
| `AIDASH_SSE_BACKPRESSURE_TIMEOUT_SECONDS` | 30 | 1–300 |
| `AIDASH_SSE_CONNECTIONS` | 128 | Existing HTTP admission bounds |

Compose reads the two new values from `.env`; the Helm equivalents are
`sse.reconcileIntervalMs` and `sse.backpressureTimeoutSeconds`. All HTTP replicas
need ordinary subscription permission for their own Node's events subject.
Inline username/password or token authentication in `NATS_URL` is decoded through
the same helper for the event Outbox, UI subscriber and worker activation;
credentials are removed from the address passed to transport diagnostics.
Do not attach the UI subscriber to an execution queue group. Worker-only
processes do not open UI subscriptions. Configure the private metrics listener
with `AIDASH_METRICS_LISTEN` as described in [HTTP observability](http-observability.md).

Broker failure permits HTTP startup and existing-stream PostgreSQL reconciliation.
Setup has a five-second overall deadline. Failed/disconnected/overflowed
subscriptions are recreated with jittered 250-ms–5-second retry delays; a
successful subscribe **and flush** triggers immediate reconciliation. A connection
callback alone does not establish readiness. The Outbox retains a 250-ms idle/error
scan interval. After publishing an event it scans every 50 ms for one second,
renewed by further successful publication, to spread sustained notification and
audit work across smaller batches. This cadence remains part of end-to-end
notification latency; Outbox SQL is measured separately from SSE event reads.

Credential/policy checks run independently every 250 ms, browser-session checks
every five seconds, and current authority is checked before each event handoff.
The original per-decision audits remain; compound checks insert their decisions
in bounded batches under the same transaction-wide allocation lock. A materialized
CTE acquires that lock as the INSERT's input, so no audit sequence is allocated
before serialization and no client round trip separates lock acquisition from
insertion. Idle checks
use a single current database snapshot without row locks or decision audits;
they only decide whether to keep the connection alive and never authorize an
event. The protected per-frame path still takes its credential/policy leases.
Losing one Workspace permission closes a selected-Workspace
stream, while global streams filter that Workspace and continue. A valid identity
with no permitted Workspaces stays connected. Permission restoration does not
rewind scan progress. Browser invalidation closes silently; other authority or
database failures retain the generic stream error contract.

A closed transaction gate is retried every 250 ms without event-log reads.
No transaction/authority lock spans an idle wait or a yielded frame. Keepalives
remain 15-second comments. Pending output without consumer progress for 30
seconds closes the logical connection and releases its admission slot, tasks,
registration and queued raw events even if the response body is never polled
again. Empty idle streams do not hit this deadline; server authorization or gate
waits do not count as slow-reader time. Bytes already handed to the HTTP transport
cannot be recalled. The browser resumes from its last received ID. SIGTERM uses
the existing 20-second drain; shared broker objects are retained.

## Diagnose delivery

| Private metric | Meaning |
| --- | --- |
| `aidash_sse_subscriber_ready` | 1 only after subscribe/flush succeeds; 0 while degraded |
| `aidash_sse_subscriber_retries_total` | Failed or lost subscription attempts |
| `aidash_sse_last_subscription_timestamp_seconds` | Most recent confirmed subscription |
| `aidash_sse_last_reconciliation_timestamp_seconds` | Most recent completed canonical page read |
| `aidash_sse_notifications_total`, `aidash_sse_rejected_notifications_total` | Accepted metadata and invalid/wrong-Node envelopes |
| `aidash_sse_coalesced_wakeups_total` | Redundant generations coalesced per observing connection |
| `aidash_sse_event_queries_total` | SSE event-query calls, including negative-cursor head lookup |
| `aidash_sse_query_causes_total{reason}` | `initial`, `notification`, `reconnect`, `fallback`, `backlog`, `gate_release` |
| `aidash_sse_authority_checks_total`, `aidash_sse_browser_checks_total` | Logical idle/initial authority and browser checks |
| `aidash_sse_visibility_checks_total` | Attempts to acquire the durable visibility gate |
| `aidash_sse_visibility_waiters` | Connections currently waiting for the gate; released on progress or cancellation |
| `aidash_sse_registered_scopes` | Active distinct local scope registrations |
| `aidash_sse_pending_pages`, `aidash_sse_pending_events` | Queued raw canonical pages/records, excluding the currently checked event |
| `aidash_sse_frames_total` | Frames handed to the HTTP body, not browser acknowledgments |
| `aidash_sse_closed_total{reason}` | `authority`, `browser`, `database`, `backpressure`, `shutdown`, `disconnected` |
| `aidash_sse_reconcile_interval_seconds`, `aidash_sse_backpressure_timeout_seconds` | Effective configuration |

A read can have several causes. Do not sum cause counters to obtain query count,
or treat a timer/reconnect-assisted read as notification-only delivery. The
event-query counter increments immediately before the database call; a failed or
cancelled call might not reach PostgreSQL. Use `pg_stat_statements` when measuring
actual backend executions, as the acceptance benchmark does. The
authority and visibility counters represent logical operations, each potentially
issuing multiple SQL statements. Existing connection/disconnect counters are
documented separately. No event, user, tenant, Workspace or credential label is
added. Subscriber failures are logged without raw envelopes or broker URLs.

If HTTP remains usable with subscriber readiness 0, inspect broker connectivity
and subject permissions while checking that fallback queries continue. With
readiness 1 but no traffic, lack of notification counts alone is expected. If
notifications rise without frames, inspect canonical backlog, effective scope and
the transaction gate; a hint is not proof that an eligible event exists. A high
pending-output count with backpressure closes points to readers/network delivery.

## Reproduce verification

```sh
scripts/test-sse-delivery.sh
scripts/test-sse-delivery.sh --benchmark
```

The opt-in benchmark builds the inspected polling baseline
`827480c13d796bca142787be5bbd25ce34c80551` from a Git archive, preserving the working
checkout. `AIDASH_SSE_BASELINE_REVISION` can explicitly select another comparison.
Both server binaries use the production `release` profile; the test controller
uses the debug profile. It uses three real API OS processes, real PostgreSQL/NATS,
100 connections across
10 Workspaces (90 filtered, 10 global), and three 30-second windows at 10 events/s.
A priming event reaches every observer before the measured window.
Fallback is delayed to 60 seconds and metric deltas reject timer/reconnect-assisted
windows. A controller-held database advisory barrier places writer A immediately
before its durable event/commit boundary. Receipt timestamps use the same monotonic
clock and complete parsed SSE frames. Complete-fan-out latency uses the slowest of
19 expected clients per event, with nearest-rank percentiles. Every repetition
requires p95 ≤ 500 ms and p99 ≤ 1000 ms; failures are retained.

Three 60-second idle windows per version use normal defaults and require at least
90% fewer actual event-log queries. The fixture enables `pg_stat_statements` and
records all SQL separately from event, authority, visibility and Outbox statements,
including per-statement execution-time deltas for diagnosing serialization waits.
It also records process CPU/RSS, raw receipts, metrics, exact source patch/hash,
baseline SHA, commands and exit status under `AIDASH_SSE_EVIDENCE_DIR` (default
`target/sse-evidence/<UTC time>`). The 95% timer-ratio estimate is not a total-SQL
or production-capacity claim. Fixture data and credentials are disposable.

The [2026-09-30 verification report](sse-delivery-results.md) records the measured
latency, query reduction, environment and exact evidence hashes.
The [acceptance coverage map](sse-delivery-acceptance.md) connects each agreed
scenario to its executable checks and implementation boundary.

The focused cases cover canonical-only hints, lost/duplicate/reordered/wrong-Node
notifications, bounded/denied replay, cursor parsing, empty/global scope changes,
buffered and idle revocation, an unread response, a held visibility gate, broker
startup/outage/recovery (including restart after loss of its event stream), and
cross-replica SIGTERM/crash replay. Existing
`authorization`, `resource_authorization`, `dashboard_oidc`, `remote_reads` and
`http_protection` suites cover the adjacent authorization and browser contracts.
The [design specification](../design/2026-09-30-sse-event-delivery-specification.md)
remains the acceptance contract; a local run does not establish hosted CI or a
production release.
