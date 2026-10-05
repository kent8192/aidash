# Worker activation operations

Issue #71 adds a dedicated activation transport for the executing Node. PostgreSQL
remains the authority for Run eligibility, visibility, leases, input observation,
and outstanding activation responsibility. Domain event routing remains separate.

## Provisioning and roles

Run `manage migrate` with the normal database configuration, then run
`manage activation-provision` with operator NATS credentials. The latter prints
the stream name, subject, durable consumer name, and creation timestamp. Use the same Node ID,
namespace, retention, and replication settings as the runtime. The command creates
missing objects and validates existing ones; it does not delete/reset consumers.
A mismatch is an operator error and puts runtime activation into recovery.

Provisioning selects only `node.node_id` and `node.nats_url` from the same composed
settings as the runtime: base/profile TOML, interpolated variables and the legacy
`AIDASH_NODE_ID` / `NATS_URL` deployment inputs. It does not require database, API
or provider secrets. `AIDASH_ACTIVATION_NATS_URL` overrides the selected broker
connection in both provisioning and runtime. The `aidash` compatibility binary
runs the same registered Reinhardt commands.

All runtime roles publish. Only `worker` and `serve` consume the shared
`workers-v1` durable pull consumer. Each free slot requests one reference and
retains its capacity until its leased durable step ends. Server-only processes
never pull or acknowledge activation messages. No consumer is tied to a Pod name.

| Configuration                        | Default         | Purpose                                                                                                          |
| ------------------------------------ | --------------- | ---------------------------------------------------------------------------------------------------------------- |
| `AIDASH_ACTIVATION_NAMESPACE`        | `default`       | Stable deployment/account namespace, combined with exact Node ID and protocol version in a SHA-256 broker scope. |
| `AIDASH_ACTIVATION_NATS_URL`         | `node.nats_url` | Optional dedicated activation connection/role credentials. Never print this URL in diagnostics.                  |
| `AIDASH_ACTIVATION_NATS_CREDENTIALS` | Unset           | Optional mounted NATS credentials file; takes precedence over URL credentials.                                   |
| `AIDASH_ACTIVATION_BOOTSTRAP`        | `false`         | Explicit local-development creation of identical objects; production provisions separately.                      |
| `AIDASH_ACTIVATION_MAX_AGE_SECONDS`  | `86400`         | Broker retention; unresolved database obligations do not expire.                                                 |
| `AIDASH_ACTIVATION_MAX_BYTES`        | `1073741824`    | File/WorkQueue capacity with DiscardNew.                                                                         |
| `AIDASH_ACTIVATION_REPLICAS`         | `1`             | Broker replicas, 1–5; operator must provision matching JetStream capacity.                                       |
| `AIDASH_WORKER_SLOTS`                | `4`             | 1–4 execution slots per process, preserving the existing database pool budget. Scale replicas for more capacity. |

The Helm chart exposes `activation.*`, `worker.slots`, and optional
`server.existingSecret` / `worker.existingSecret`. Role secrets may contain a
separate `AIDASH_ACTIVATION_NATS_URL`. Each role override is a complete runtime
Secret and must also include the shared database, API and provider settings.
Provider and domain-event permissions remain
independent. Database readiness and the existing 20-second drain/30-second Pod
termination grace period are unchanged; broker failure does not gate readiness.

Provision using an operator identity that can manage the exact activation stream
and durable. Runtime publishers require the exact subject, stream-info API, and
request/reply inbox access. Workers additionally require consumer-info and
`$JS.API.CONSUMER.MSG.NEXT.<stream>.workers-v1`, the exact consumer ACK subjects,
and their reply inboxes. Deny worker-pull and ACK subjects to server identities.
Do not grant runtime consumer deletion/management to repair mismatches. Use NATS
account isolation and explicit subject permissions; wildcard grants spanning other
Nodes violate this boundary. Production replication, account sizing, TLS, and
credential issuance are deployment responsibilities.

## Persistence and recovery

`run_activations` is an append-only obligation/disposition journal with independent
requested generations. Generations are allocated by a database sequence; gaps do
not imply settlement. Run/input, Task dependency completion, human response, approval,
and core admission/ordering triggers append in the writer's transaction, including
compatible old writers. Ordering release notifies only the next nonterminal Run
after the head completes, preserving paused predecessors. Transaction visibility
gates retain existing obligations; publication retries and consumer redelivery resume
after release without appending notifications for unrelated Runs.
`activation::request_in` is also available for #70's
transactional recipient handoff. Trigger/function DDL is the documented SeaQuery
exception; runtime queries use SeaQuery. No recipient selection is performed here.

A message references protocol, Node, Run, activation ID, and generation. A worker
reloads state under the visibility gate and commits either a named Run lease or a
recorded terminal/superseded/deferred disposition before ACK. New input is never
marked observed by settling scheduling responsibility. Independent future deadlines
and newer inputs retain independent rows. External effects continue through the
existing execution authority, lease fencing, and replay-safety checks.

Publish claims expire after five seconds. Uncertain publish retries retain the same
`Nats-Msg-Id`; intentional re-notification increments the durable publication epoch.
The indexed 250-ms due scheduler handles wake/retry/approval/owner-expiry deadlines.
Published, unclaimed obligations are re-notified after five seconds, so broker expiry
or storage loss cannot make publish confirmation into permanent settlement.

Workers perform startup recovery and bounded broad recovery every five seconds,
or every second during known transport loss. Connection attempts run separately,
with a five-second deadline and exponential backoff from 500 ms to five seconds
including jitter. A dead owner's existing 30-second lease must expire first.
Compatible schema is retained on binary rollback. Before enabling incompatible
future Run semantics, stop incompatible old workers; a notification is not a
compatibility upgrade mechanism.

On shutdown, stop new pulls/claims and drain the current step. Do not clear a lease
while an external effect might still run. Never delete the durable on shutdown.
`activation_quarantine` stores only a digest, bounded reason, sequence, and time;
persistence precedes TERM and arbitrary payloads are not retained.

## Diagnostics and verification

Private metrics include `aidash_activation_mode{mode=...}` with `recovering`,
`event_driven`, `fallback`, or `stopping`; claims by `notification`/`recovery`;
publish/ACK/defer/quarantine counts; oldest due age; and last successful recovery
scan time. Mode readiness is distinct from observed publication/consumption
progress. Logs correlate committed claims with activation generation and worker
PID. Do not use successful broker connection as execution proof.

Run `bash scripts/test-worker-activation.sh` for real PostgreSQL/JetStream,
separate executable processes, scripted inference, and raw causal timings. The
runner records Git SHA, dirty patch hash, new-file hashes, commands, exit status,
PIDs, and per-sample upper-bound admission-to-lease timings under
`target/activation-evidence/`. This is component evidence, not production scale or
#70 end-to-end routing evidence. Broader regressions use `scripts/test-rust.sh`;
cluster deployments use `scripts/test-cluster.sh kubernetes` and `k3s`.

Only with `AIDASH_ENV=test`, `AIDASH_ACTIVATION_TEST_RECOVERY_MS` can delay recovery
up to 60 seconds and `AIDASH_ACTIVATION_TEST_PAUSE_FILE` can suppress consumption
while that file exists. `AIDASH_ACTIVATION_TEST_AFTER_ACK_PAUSE_FILE` pauses a claimed step after ACK
and before execution for the crash/lease-expiry test. These fault controls leave
publishers and durable state active. They are rejected outside an explicitly identified test environment.

See [verification evidence and remaining acceptance gaps](worker-activation-verification.md)
for the exact exercised scenarios and source identity.
