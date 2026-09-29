# FR-TX-001 transaction evidence register

Status: implementation and focused acceptance evidence, **not complete release acceptance**. Issue [#40](https://github.com/kent8192/aidash/issues/40) remains open. The [accepted design](../design/2026-09-28-transaction-acceptance.md) is the requirements baseline, and the [transaction contract](../transactions.md) describes implemented behavior.

## Implemented boundaries

Subject submission uses immutable source/receiver bindings, metadata-only disclosure preflights, live source and mapped Participant admission checks, durable unknown admission attempts, and current authorization for full status and undecided abort. Registry registration and trust/transport recovery remain operator-only. Authority controls can operate during a Node-wide visibility reservation; the database exception cannot write ordinary application data. Active and aborted recovery have separate loops and connection capacity.

The migration is additive and chronological. Its trigger-function DDL is documented as a SeaQuery exception. It refuses downgrade with pending coordinator or participant work. The opt-in fault hooks require a process-owner-selected UUID, point and filesystem directory; ordinary deployments perform no hook filesystem I/O and expose no fault-control endpoint.

## Executed local checks

| Check                          | Scope                                                                                                                                                                             | Result                                                                                                        |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `scripts/test-transactions.sh` | 64 protocol cases, 10 local authority cases, one actual scoped Home/Executor finalization case                                                                                    | 75 passed; zero failures                                                                                      |
| OS process matrix              | 24 cases, three repetitions each: before/after coordinator submission, vote, COMMIT, ABORT, visibility and completion; participant reservation, prepare, apply, release and abort | 72 repetitions passed; ordinary visibility, convergence, single revisions/events and cleared barriers checked |
| Three/sixteen-Node tiers       | Independent databases; COMMIT and ABORT with one participant unavailable; three repetitions per case                                                                              | Passed                                                                                                        |
| Two/three-Node authority       | Source, receiver, mapping and trust revocation before/after reservation; lost durable reservation reply; unauthorized recipient disclosure                                        | Passed; unknown revocation returns 202, reconciled revocation returns 200                                     |
| Mutation authority             | Workspace actions/disclosure, Task/Run stored-chain intersection, actual remote execution admission, same-peer restoration with trust still disabled                              | Passed                                                                                                        |
| Authorization regression       | `authorization`, `execution_authorization`, `peer_authorization`, `resource_authorization`                                                                                        | 31 passed                                                                                                     |
| Browser contract               | Subject controls across barrier/reload, denied detail removal, hidden operator controls, pending revocation                                                                       | Two Playwright cases passed against the production web build; APIs are explicit UI fixtures                   |
| Build/lint                     | API generation, TypeScript/Vite production build, Clippy with warnings denied, Trunk                                                                                              | Passed: production build, Clippy `-D warnings`, and Trunk (Clippy checked separately)                         |

The actual remote finalization case uses the existing scoped-execution fixture with isolated PostgreSQL schemas and real peer HTTP. It is component authorization evidence; the transaction protocol/tier suites and cluster driver use separate databases for each Node. The existing operator dashboard end-to-end scenario remains in `web/tests/transactions.spec.ts`; the new UI fixtures do not claim to have rerun that real-backend browser scenario.

## Cluster procedure and identity

Run `scripts/test-cluster.sh kubernetes transactions` and `scripts/test-cluster.sh k3s transactions`. CI runs these separately from the existing platform profile. The standalone Python driver accepts only an explicit kubeconfig, image and SeaQuery-generated diagnostic queries, creates a disposable namespace and removes it afterward. It never changes the user's current context. Fixture credential values are not part of its evidence output.

The local run uses Kubernetes `v1.35.6+orb1` and k3s `v1.35.5+k3s1` on Docker/macOS ARM64. Each distribution provisions sixteen distinct Node IDs, database names, database roles and per-peer credential values. One PostgreSQL 17 server with a persistent volume hosts those independent databases; PostgreSQL failover, physical-server isolation and durable-data loss are not tested. NATS is a fixture dependency. These are backend transaction fixtures without provider, Tool, Qdrant, TLS or browser traffic and without an availability/throughput claim.

The tested image is `aidash:issue-40-final`, manifest-list digest `sha256:48b658c59cd30f74f5c93dde0296bc25fd003e43bc7050a6d17d84e53194560e`. The source-input fingerprint is `361e4ea5d73fa7bc6e62d9df9a14877e024a31fd361da78f41934974c1498f9d`; `source-inputs.json` records each Rust/Cargo/Dockerfile input hash. The build used uncommitted implementation changes above `20d1fb46`, not the unmodified base revision. The subsequent fast-forward to `6b90aca` changed only unrelated Creator documents/UI. The supplemental image `aidash:issue-40-recovery-verified` includes the final peer-recovery audit-kind correction: manifest-list digest `sha256:8a794fec19e8a4f77896022c335bf7898dfa2a6e0d58e60a60db8b5f6a75998b`, source-input fingerprint `5616b4d98bfa412e82762b99818b74d5a1fa0357742d7957bd0ac320a04bbe23` above `6b90aca`. Its two-Node scaling and peer-recovery repetitions passed on both distributions. This is focused final-image verification; the entire main matrix was not rerun on that image.

Each case records UUID, Node count, phase/edge, outcome branch, repetition, held fault cut, fault removal, service restoration, convergence observation, ordinary HTTP observations, and final participant/barrier/workspace/event rows. New ordinary state requires every participating database to have applied; unavailable responses are allowed during the barrier. Convergence is bounded to 180 seconds **after fault removal and required service restoration**, not an RTO from the initial fault. Worker scaling exercises zero, two and zero replicas; rolling replacement uses the same image. A partial-partition fixture stops one participant service while retaining its database. This is service unavailability, not packet-filter or asymmetric-network fault evidence.

The complete driver inventory is 105 repetitions per distribution: 72 two-Node durable-cut repetitions, nine two-Node lifecycle repetitions (including same-peer authentication restoration), and twelve each at three/sixteen Nodes for COMMIT/ABORT unavailability, scaling and rolling replacement. The original main inventory contains 102 repetitions; a supplemental final-image run repeats the two-Node scaling cases and adds the three peer-restoration cases with explicit zero-worker assertions. Retained run records identify actual passes and failures; a planned row alone is never a passed gate.

## Recorded cluster outcomes

The [evidence bundle](evidence/2026-09-28-transactions/README.md) includes compressed original traces, their uncompressed SHA-256 values, source-input manifests and a machine-readable summary. Runtime inputs in implementation commit `d4cf0cf1ed316b7ce58260cc71d0a1456715fe3e` match the final-image fingerprint above.

| Distribution / run                                | Image                        | Actual outcome                              | Maximum observed convergence       |
| ------------------------------------------------- | ---------------------------- | ------------------------------------------- | ---------------------------------- |
| Kubernetes main, `dbd3e9dda3`                     | `issue-40-final`             | 86 passed, one failed, remaining 15 not run | 13.207 s among passing repetitions |
| Kubernetes two-Node supplement, `5b825a8c34`      | `issue-40-recovery-verified` | Six passed                                  | 10.827 s                           |
| Kubernetes three/sixteen-Node rerun, `3187e44fa1` | `issue-40-recovery-verified` | 24 passed                                   | 23.517 s                           |
| k3s main, `1f7379854a`                            | `issue-40-final`             | 102 passed                                  | 31.440 s                           |
| k3s two-Node supplement, `ff24b4e22e`             | `issue-40-recovery-verified` | Six passed                                  | 10.121 s                           |

Each distribution has passing evidence for all 105 distinct repetitions of the implemented inventory across these identified runs. Repeated cases retain separate records; the Kubernetes main-run failure remains failed. These results cover the executable subset above, not all accepted release gates or a single final-image acceptance run. Task-owned namespaces, port forwards and the temporary k3s cluster were removed after verification.

## Review findings affecting historical evidence

The September 28 cluster records predate two corrections to the acceptance driver. Their peer-recovery lifecycle revoked admission trust without disabling peer communication, so those historical repetitions do **not** prove authentication restoration. Their visibility oracle checked every database when it observed revision 1, but accepted a later revision-0 API response. They therefore do **not** establish monotonic ordinary API visibility after publication. The retained raw outcomes above remain historical records, with these limitations; they are not upgraded by changing the driver.

The corrected driver remembers publication across the entire case trace and rejects every later successful old-state response, including responses from another Node. Its peer-recovery lifecycle disables the existing peer, removes the process fault, observes a failed recovery attempt and visibility barriers, and only then restores that same peer through the operator endpoint while admission trust remains disabled.

The [September 29 review-repair bundle](evidence/2026-09-29-transaction-review/README.md) records three passing two-Node Kubernetes `v1.34.0` repetitions of the corrected peer-recovery lifecycle. The image is `aidash:pr87-review-20260929`, with runtime fingerprint `c25de28fa9b2b8e95c79ec3fb3e115f69c42db733bdaf4dec3e34e8f3163cee0`; image identities, exact input hashes, driver hash and original traces are retained in the bundle. Maximum observed convergence was 1.039 seconds after required service restoration. Local review verification also passed 20 Rust cases, five UI cases, four visibility-oracle regressions, the production web build, Clippy with warnings denied and Trunk. No full acceptance-matrix rerun or new k3s run is claimed for this repair.

## Failure and rerun ledger

Earlier failed executions are retained separately from successful reruns:

- A shared machine-wide Cargo target launched another worktree's executable, whose migration inventory was incompatible. Real-process tests now select this checkout's `AIDASH_TEST_BINARY` explicitly.
- Early process probes exceeded the fixture's HTTP rate limit. Only test-server burst limits were raised; production limits were unchanged. A later 500 ms observation timeout under concurrent builds was replaced with a five-second test request bound, since the design does not impose a 500 ms latency target.
- Early tier fixtures used invalid database-name hyphens, reused peer credentials, or used credentials shorter than the existing minimum. The fixtures now use valid database names and distinct sufficiently long test credentials.
- Two simultaneous sixteen-Node tests exhausted PostgreSQL's default 100 connections. The disposable Compose test database now permits 600 connections.
- A later Kubernetes main run passed 86 repetitions, then missed `coordinator.commit.before` on the third three-Node scale repetition. The observed transaction committed, so the intended crash was not exercised and that repetition is failed. The old driver did not wait for all terminating Workers to disappear before the next case. The updated driver verifies zero Worker Pods; the three/sixteen-Node tiers passed all 24 repetitions when rerun separately on the final image.
- A Kubernetes run missed a fault cut because `kubectl port-forward` remained attached to a terminating Pod with old fault settings. The driver now reconnects after every server rollout. The failed run remains failed.
- A k3s provisioning attempt failed to connect during PostgreSQL's temporary initialization server. Readiness now probes TCP, which becomes available only for the final server. One earlier k3s run was interrupted while replacing the driver/image and is incomplete.
- Two supplemental fixtures and a diagnostic retry failed at Peer registration with HTTP 500 before any transaction case ran. Retained reruns passed; the driver now waits for the peer identity endpoint from the originating Pod before registration, covering Service/DNS propagation separately from Pod health. These setup failures are not successful transaction cases.
- The new peer-recovery regression found an invalid audit role rejected by the existing database constraint. Recovery now records the existing `trust` role with an explicit `AUTHENTICATION_RESTORED` phase; the regression passes and proves that admission trust remains disabled.

## Unpassed release gates

The implemented checks do not close every row of the agreed inventory. Keep these explicit:

1. Source authority-attempt persistence, authority-proof response and revocation-completion process cuts exist in part, but the complete lost/reordered-response and crash matrix at every authority/revocation step has not run on both distributions.
2. Two/three-Node scoped authority, disclosure and trust/mapping cases are Rust integration evidence. They have not all been repeated as real cluster-process races, and packet-level bidirectional/asymmetric partitions remain untested.
3. Worker scaling and same-image rolling replacement are exercised; the full Worker-owned durable-cut matrix and reserved/prepared/COMMIT coexistence with an actually different compatible version remain unqualified. No distinct compatible predecessor is claimed.
4. The entire acceptance inventory must pass on one exact release candidate. The main image and focused final-image supplement are recorded separately; hosted CI is a separate fact tied to the PR's exact head.
5. Permanent state loss and inconsistent backup restoration belong to Issue #73. They never permit a Participant to guess ABORT or replace an irrevocable COMMIT.

The PR delivers the implementation, repeatable checks and evidence boundaries. Closing Issue #40 requires passing evidence for every applicable accepted gate above; neither local tests nor same-version replacement imply that closure.
