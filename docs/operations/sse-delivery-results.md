# SSE delivery verification — 2026-09-30

Local acceptance passed for Issue #72 on the workload below. The full run exited 0: 19 focused functional cases and the opt-in acceptance case passed. All three latency windows and all three idle comparisons met their individual thresholds.

## Tested build and environment

- Baseline: `827480c13d796bca142787be5bbd25ce34c80551` (polling implementation).
- Candidate source was captured before commit on that base, branch `feat/issue-72-sse-delivery`; the hashes below identify the tested implementation.
- Both API binaries: production `release` profile. Test controller: debug profile.
- Native host: 14 logical CPUs, 48 GiB RAM, `Mac16,11`. Platform: `macOS-26.6.2-arm64-arm-64bit`; `rustc 1.96.0 (ac68faa20 2026-05-25)`.
- Docker: `CPUs=14 MemoryBytes=16818978816`; no per-process resource limits.
- Services: PostgreSQL 17 with pg_jsonschema 0.3.4 and pg_stat_statements, NATS 2.12 Alpine, Qdrant 1.19.1. Exact image IDs/digests are in the invocation manifest.
- Three native API processes; 100 SSE connections across ten Workspaces, distributed 34/33/33 between replicas: 90 Workspace-filtered and ten global observers. Admission metrics confirm all 100 connections before and after each window.

## Active delivery

Three independently prepared 30-second windows send ten HTTP Workspace mutations per second through replica A. Each event must reach its nine matching Workspace observers and all ten global observers. Every expected pair was received exactly once with increasing canonical IDs on each connection: 300 events and 5,700 receipts per window.

| Repetition | p50 (ms) | p95 (ms) | p99 (ms) | Maximum (ms) | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 62.55 | 236.44 | 263.89 | 278.82 | Pass |
| 2 | 58.82 | 235.53 | 260.38 | 354.80 | Pass |
| 3 | 55.57 | 229.25 | 274.61 | 432.70 | Pass |

The distribution uses each event’s slowest expected client receipt, with nearest-rank percentiles. The shared monotonic clock starts just before releasing writer A’s database event/commit barrier and stops at complete SSE frame parsing. This conservatively includes the remaining commit and transport overhead. Every window met p95 ≤ 500 ms and p99 ≤ 1,000 ms.

Fallback was configured to 60 seconds. On every replica, initial/fallback/reconnect read counters were unchanged during the window while notification-caused reads increased. No missing, duplicate, wrong-scope or regressing frame was observed.

Active SQL per 30-second candidate window (classification is the same as the idle table):

| Repetition | All SQL | Event reads | Authority SQL | Visibility SQL | Outbox SQL |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 150,725 | 5,654 | 53,594 | 14,231 | 1,802 |
| 2 | 150,511 | 5,640 | 53,525 | 14,207 | 1,795 |
| 3 | 150,985 | 5,671 | 53,617 | 14,280 | 1,835 |

## Idle event queries

Each version uses the same fixture and connection distribution for three 60-second idle windows, excluding setup and priming. The candidate uses its normal five-second reconciliation. Counts are PostgreSQL pg_stat_statements deltas for actual event SELECTs; no event arrived during an idle window.

| Repetition | Baseline queries | Candidate queries | Reduction | Candidate / connection / second |
| --- | ---: | ---: | ---: | ---: |
| 1 | 22,544 | 1,200 | 94.68% | 0.20 |
| 2 | 23,283 | 1,200 | 94.85% | 0.20 |
| 3 | 23,079 | 1,200 | 94.80% | 0.20 |

Every repetition exceeded the 90% event-query reduction requirement. Other SQL is reported separately below (baseline → candidate, statements per window). SQL categories are text-based classifications of captured statements and are not additive component totals; all SQL includes transaction/control and harness work.

| Repetition | All SQL | Authority SQL | Visibility SQL | Outbox SQL |
| --- | ---: | ---: | ---: | ---: |
| 1 | 282,693 → 50,594 | 110,498 → 27,605 | 25,347 → 4,052 | 686 → 708 |
| 2 | 291,628 → 50,609 | 114,069 → 27,607 | 26,132 → 4,055 | 706 → 708 |
| 3 | 289,149 → 50,598 | 113,061 → 27,609 | 25,918 → 4,052 | 702 → 708 |

## Sampled process resources

| Workload | Highest sampled CPU per API process | Highest sampled RSS per API process |
| --- | ---: | ---: |
| Active candidate | 10.4% | 39.72 MiB |
| Idle baseline | 8.1% | 38.05 MiB |
| Idle candidate | 4.9% | 38.89 MiB |

CPU values are host ps samples, not enforced limits or capacity estimates. Raw snapshots include cumulative CPU time. PostgreSQL and NATS run inside the shared Docker engine.

The oversized process case delivered 1,049,600 data bytes through a broker with a 1,048,576-byte message limit using a reference notification. The full canonical payload matched; sampled API RSS changed from 35.03 to 42.39 MiB.

## Evidence and scope

The [repository evidence bundle](evidence/2026-09-30-sse-delivery/README.md) includes the measured source hashes, all nine SQL/metrics/resource windows, all 17,100 receipts and process assertions.

Reproduce with `scripts/test-sse-delivery.sh --benchmark`. The local raw bundle is at `/Volumes/cache/aidash-sse-evidence-20260930-final-publication`: invocation settings and image metadata, source patch and untracked source copies, native binaries, all per-client/per-replica receipts, SQL rows, metrics, CPU/RSS snapshots, functional assertions and process logs. `verified-inputs.json` confirms the captured source and both binaries still matched after measurement. Subsequent report-only edits do not change those tested runtime inputs.

- Baseline binary SHA-256: `8965c712c0386b70fa45d42f40f1c8f511d494726f3b6d83923291ee81a29032`.
- Candidate binary SHA-256: `3146622e3628d915e54c83da21cecadfe47fdd1d9158ba17d08c92672739f8b2`.
- Tracked source patch SHA-256: `b8081225cd7690e49558c3ef93b94df6ed22f06f29030b9b3ca942351eba9cab`. Untracked source hashes are in `invocation.json`.

Additional local verification: 155 library tests; authorization (8), dashboard OIDC (4), resource authorization (12), remote reads (1), HTTP protection (2), and focused SSE cases (19). The focused cases include broker-unavailable startup, broker restart after deleting its event stream, canonical fallback, independent execution ACKs, and replay after SIGTERM or forced process death. Three additional Outbox, malformed-event and activation-permission regression tests passed. Commands and logs are retained with the bundle.

Final Clippy with warnings denied, Rust formatting, shell syntax and documentation-link checks also passed. The deployment values and schema were checked with Helm lint and template rendering.

Exploratory debug-server runs are retained at `/Volumes/cache/aidash-sse-evidence-20260930`, `...-final` and `...-v3`. Each had a latency-threshold failure; the first two preceded the completed authorization-query changes. Those are not passing release evidence. The harness was then corrected to test the production release profile. A source-change build guard also stopped an earlier build attempt before acceptance; its log and invocation are retained in `/Volumes/cache/aidash-sse-evidence-20260930-release`. That earlier release run passed before the additional authority/permission coverage, waiting metric and shared broker authentication were completed; this report records the final rerun.

Two later release-profile attempts also failed latency acceptance and remain at `/Volumes/cache/aidash-sse-evidence-20260930-publication` (p95 851.79 ms, p99 1088.97 ms) and `...-diagnostics` (p95 1278.40 ms, p99 1583.46 ms). The diagnostic run recorded 411.37 accumulated seconds in 6,600 advisory-lock calls; ordinary queries were each under one accumulated second. The final implementation combines audit-lock acquisition with INSERT to remove a serialized client round trip, and adds a real blocked-frame audit-sequence test. The audit-only release run at `/Volumes/cache/aidash-sse-evidence-20260930-audit` improved to p95 532.32 ms / p99 702.66 ms but still failed p95. The final publisher retains 250-ms idle/error scans and uses 50-ms scans for one second after a successful publish to spread sustained bursts. The thresholds and authority checks were retained. Other workloads were active on the shared host during diagnosis, so results describe the recorded local environment.

These results establish the specified local workload and fault cases. They do not establish hosted CI, Issue #73 production capacity, publication or deployment. See the [acceptance coverage map](sse-delivery-acceptance.md), [operations runbook](sse-delivery.md) for behavior and settings, and the [accepted specification](../design/2026-09-30-sse-event-delivery-specification.md) for the contract.
