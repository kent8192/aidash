# Worker activation verification

This is component evidence for Issue #71, not a declaration that every scenario
in the design acceptance matrix has passed. Validation used the working tree on
`docs/issue-71-worker-activation-design`, based on
`20d1fb46f13cf042af6afce7763140c132428b78`, before its implementation commit.
The recorded dirty-tree status describes that validation snapshot. Publishing
these results does not establish hosted CI or a production rollout.

## Reproduction and source identity

```sh
bash scripts/test-worker-activation.sh
RUST_TEST_THREADS=2 bash scripts/test-rust.sh
RUSTC_WRAPPER= cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
bash scripts/test-cluster.sh kubernetes
bash scripts/test-cluster.sh k3s
```

The process runner records the base commit, dirty status, exact patch and SHA-256,
untracked-file hashes, command, exit status, per-process logs and raw per-sample
results. Cluster reports contain image IDs and a complete build-input hash manifest.
Use those artifacts to identify a tested dirty checkout; the base commit alone
is insufficient. Test credentials are disposable local fixture values.

Services are real PostgreSQL 17 with pg_jsonschema 0.3.4 and NATS 2.12. The process
fixture uses two slots per worker, distinct executable PIDs and scripted HTTP
inference. Each latency sample uses a fresh workspace to keep history size constant.
The 100 samples run in five windows with startup barriers and a 60-second recovery
delay. Each window checks zero recovery claims. Timings start before the HTTP
admission request and end after observing its first committed lease, giving a
conservative admission-to-lease upper bound. No provider time is subtracted.

## Recorded local results — 2026-09-28

[Machine-readable results](worker-activation-results.json) identify commands,
source manifests, raw logs, PIDs, image IDs and remaining validation. All 255
runtime, configuration and test inputs checked against the final test manifest
were unchanged when implementation commit `17bad16` was published. Later review
repairs change runtime, chart and test inputs; the historical cluster images and
latency measurements below do not validate those repairs.

| Validation                    | Observed result                                                                                                                                                             |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust unit/integration targets | **512 cases passed**, across the full-suite attempt, targeted fixture retry and remaining targets.                                                                          |
| Final activation fixture      | 5 tests passed; 100 timing samples; **p95 174.6 ms**, **maximum 746.1 ms**; zero measured recovery claims; 107 expected provider completions including control/batch cases. |
| Formatting and lint           | `cargo fmt --check`, Clippy with `-D warnings`, shell syntax and Python compilation passed.                                                                                 |
| Kubernetes                    | v1.34.0; Pod kill/recovery, scale zero/up, rolling restart, durable identity and dashboard checks passed.                                                                   |
| k3s                           | v1.34.11+k3s1; the same acceptance checks passed.                                                                                                                           |

Both cluster runs used backend image ID
`sha256:40ac5a5179076c063406cbbc1e37c1c38b979850a3a36eaf21b185e06fa1d79c`
and build-input digest
`dd623814c79055adc30d59cc26ec760a1a13bac9f87ebeb799ab0a370d9568f6`.
The source/image evidence is retained under `.ignore/platform/aidash-ops-39c524c62f/`
(Kubernetes) and `.ignore/platform/aidash-ops-de219a1fed/` (k3s).
Process evidence is under `target/activation-remaining/`.

The complete Rust command did **not** finish cleanly in a single invocation:
its two-thread run reached a Docker host-port collision while starting the
restartable Qdrant fixture, before application assertions. That exact test then
passed, and the four targets Cargo had not reached were run successfully.
An earlier four-thread run had two HTTP 500 failures in existing retention tests;
all 12 retention cases passed serially and all 57 core-capability cases passed
in the two-thread run. The precise cause of the earlier HTTP 500s is not established.
Those failed logs are preserved; these results do not claim that high-concurrency
fixture reliability is fixed. Earlier k3s setup attempts also encountered a killed
validation tool and a host-port collision; completed cluster runs were separate,
successful executions. Disposable clusters and the failed Qdrant container were removed.

## Review repair verification — 2026-09-29

The review repairs centralize the dedicated broker override, drain startup
reconciliation in bounded batches, reset reconnect delay on successful setup,
and validate the effective Secret for each Helm role. Trunk formatting and an
unused Python assignment were also corrected.

- The activation target passed **7 tests**, including 257-Run startup backfill
  and an isolated subprocess exercising `Harness::run_worker_until` with a
  dedicated broker. Its ignored helper is executed by that subprocess test.
- Clippy with warnings denied, Rust formatting and full-tree Trunk checks passed.
- Helm rendering passed for role-only, shared and mixed Secret settings; missing
  required role Secrets were rejected in three negative configurations.

These focused checks cover the review repair. The historical cluster images and
latency results remain the earlier snapshot, not measurements of this revision.

## Exercised behavior

- Server publishing and independent worker claiming; a negative control with a
  healthy publisher and consumption paused; 100 successful sequential admissions.
- Combined mode plus another worker; six concurrent admissions while another
  replica is SIGSTOPed; database fencing and worker-PID attribution.
- Atomic rollback of Run/control plus activation; wake deadlines with recovery
  delayed; duplicate delivery bypassing broker deduplication; four quarantine cases.
- Accepted input during active inference: another worker durably defers it,
  observation remains unchanged, the stale response is rejected, and re-inference
  observes the input before completion.
- An actual SIGKILL after the lease commit and broker ACK, before execution:
  replacement workers wait for the real 30-second lease to expire, then execute
  once. Database lease timestamps are not shortened by the test.
- Broker unavailable at startup, database fallback, reconnect, and deletion and
  recreation of the disposable activation stream/consumer. A disconnect event
  forces setup validation even when automatic client reconnect was brief.
- Forced DiscardNew capacity refusal and shortened retention: accepted work
  survives, then finishes by renewed notification with broad recovery delayed.
- Scoped NATS identities: server publication allowed, consumer inspection/pull
  denied; worker pull/ACK allowed; other-Node scope denied; configuration mismatch
  does not reset the durable consumer.
- Dependency completion produces an activation in the existing queued-executor
  regression fixture. Public admission and legacy/scoped authorization rules are
  preserved; activation does not bypass an incomplete-dependency rejection.
- Existing authorization, visibility, remote admission/input, unsafe-effect and
  migration tests are retained. These exercise shared execution paths; they are
  not all independent-process activation fault injections.

## Acceptance coverage and remaining gaps

| IDs       | Evidence and limits                                                                                                                                                                           |
| --------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| AT01–AT03 | Process notification, negative control, combined/split topology and PID evidence.                                                                                                             |
| AT04      | Blocked replica and batch exceeding the available two slots. Exhaustive per-slot occupancy instrumentation is not included.                                                                   |
| AT05–AT06 | Duplicate references, stale inference and concurrent input. No literal 120-second duplicate-window aging or complete out-of-order permutation matrix.                                         |
| AT07–AT09 | Atomic rollback, wake deadline and dependency release; existing control, approval, authority and visibility regressions. Not every trigger/gate timing is injected across separate processes. |
| AT10      | After-ACK/before-execution SIGKILL and cluster after-effect recovery. Not every publication/commit/ACK ambiguity checkpoint has a deterministic fault fixture.                                |
| AT11–AT12 | Startup outage, reconnect, capacity refusal, expiry and empty stream reconstruction. Deleting/recreating the stream is not a whole NATS server disk-loss restart.                             |
| AT13      | Malformed, wrong-Node, unsupported-version and mismatched-reference quarantine. Quarantine-write database failure remains untested.                                                           |
| AT14      | Active-owner deferral, input preservation, release and real lease-expiry recovery.                                                                                                            |
| AT15      | SIGTERM, SIGKILL, rolling restart and scale zero/up. Different-version coexistence and binary rollback are not exercised.                                                                     |
| AT16      | Mode logs, configuration validation and exact-scope credential tests. No production credential issuance or complete metrics scrape assertion.                                                 |
| AT17      | Atomic `activation::request_in` helper and compatibility triggers; existing #38 coverage. Real #70 recipient routing is pending its implementation/integration.                               |
| AT18      | Disposable Kubernetes/k3s configuration and image evidence. Cluster fixtures use operator NATS credentials; scoped ACL checks are separate component tests.                                   |

Do not close the broader matrix or claim #70 end-to-end routing based only on
these component results. General activation-journal retention remains with #73.
