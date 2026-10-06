# Issue #125 implementation and verification

The accepted core is implemented in the dedicated
`docs/issue-125-safe-memory-design` worktree on
`feat/issue-125-native-hindsight-memory`. Local implementation and acceptance
verification are complete for the profiles recorded below.
The branch merged `develop/0.1.0` at
`6b886aac2a29c398c7da8f771bd1b8c644d196be`; merge HEAD is
`5a8e32fb3c4457f7f11717863e2d514558b53063`. These SHAs identify the merged
verification base. Tests ran before publication; recorded source/image/driver
hashes identify the tested implementation. The commit carrying this ledger and
the PR's current head identify the published source. Hosted CI is separate from
these local results and must be checked against that head.

## Coverage and ownership

The [memory operation contract](semantic-memory.md) and
[pinned upstream mapping](../crates/aidash-domain/src/memory/UPSTREAM.md)
describe the semantic Rust translation. Python HTTP/API compatibility, Oracle,
SDKs, upstream UI and multimodal parity are outside this accepted core.

| Contract                 | Implementation and evidence                                                                                                                                                                                                                                                                                                                                         |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Retain and learning      | Application/domain extraction and graph rules plus native learning repositories. Complete canonical Run journals and immutable Task snapshots produce unverified human-review candidates. Native tests cover uncertain effects, reference-only results, failure/output evidence, exact admission/rejection replay and preserved Run lineage after human correction. |
| Recall and Reflect       | Four semantic/PGroonga keyword/graph/temporal arms, fusion and explicit reranker/tokenizer roles. Tests cover Japanese, nonlexical matches, finite scoped vectors, bounded traversal, whole-envelope budgets, delivered citations, empty/disabled/no-space/unavailable results and model memoization.                                                               |
| Derived maintenance      | Pure consolidation and native durable jobs cover entity/semantic groups, contradictory/transitive support, selective question refresh and source-change invalidation. Automatic maintenance consumes admitted memory and cannot admit new Run learning.                                                                                                             |
| Ownership and mutation   | Host-bound logical Agent/private and Workspace/shared banks, six exact Registry roles, granular CAS, request identities, deletion fences, publication/history/impact and current authority. No whole-bank replacement, legacy JSON reader or import.                                                                                                                |
| Persistence and recovery | PostgreSQL 17, pgvector 0.8.7, PGroonga 4.0.9/Groonga 16.1.2, pg_jsonschema 0.3.4. Reinhardt queries/migrations own persistence; documented raw DDL exceptions install extension objects. Real tests exercise crash recovery, reindexing, archive expiry, revision/body/deletion floors, missing/corrupt ledger rejection and restore-process SIGKILL.              |
| Cleanup and remote use   | Home-only retrieval, two-node authority/origin accounting, TTL before disclosure/replay and finite durable Home/receiver cleanup. Real FK failure proves three-attempt exhaustion, fairness and adapter recreation. Receiver management exposes content-free pending/failed/purged state without hidden Run detail access.                                          |
| Registry and UI          | en-US/ja-JP private/shared units, provenance, verification, granular mutations, CAS/retry, history/impact, review/edit/reject, question freshness, role pins and cleanup state. Component fixtures and real Home-backed browser review have separate evidence below.                                                                                                |

## Recorded checks

Logs are local session evidence. Full suites below precede the final localized
rejection-replay fix; its real-store regression is listed separately. A focused
result does not imply an unrelated full-suite result.

| Check                               | Result                                                     | Evidence and scope                                                                                                                                                                                                                                                                                                  |
| ----------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Portable domain/application/harness | 3,718 passed: application 2,829, domain 837, harness 52    | `/tmp/aidash-125-foundation-21.log`; completion drain and source/receiver TTL rules. Later portable edits were Clippy style fixes.                                                                                                                                                                                  |
| Native PostgreSQL persistence       | 38 passed, 291.42 seconds                                  | `/tmp/aidash-125-native-49.log`; canonical timestamp/replay/restore, complete learning/lineage, recovery/SIGKILL, TTL and bounded fair purge.                                                                                                                                                                       |
| Scoped remote execution             | 72 passed, 1,130.09 seconds                                | `/tmp/aidash-125-native-all-46.log`; later receiver-management DTO/timestamp changes have focused evidence below.                                                                                                                                                                                                   |
| Current native remote paths         | 3 passed, 87.92 seconds                                    | `/tmp/aidash-125-native-remote-13.log`; ordinary/generated required-Home context and content-free receiver cleanup management.                                                                                                                                                                                      |
| Generated semantic execution        | 11 passed, 171.26 seconds                                  | First suite in `/tmp/aidash-125-native-all-44.log`; its subsequent older persistence suite failed and is superseded by native-49.                                                                                                                                                                                   |
| Candidate rejection replay          | 1 passed, 7.53 seconds                                     | `/tmp/aidash-125-review-replay-50.log`; actual JSONB null, identical replay and changed-input key conflict in the complete canonical Run learning fixture.                                                                                                                                                          |
| Clippy                              | Passed with `-D warnings`                                  | `/tmp/aidash-125-clippy-5.log`; includes rejection replay and final migration formatting.                                                                                                                                                                                                                           |
| UI fixtures                         | 59 passed; current native memory 10 passed                 | `/tmp/aidash-125-ui-8.log`, `/tmp/aidash-125-ui-11.log`; latest native tests also prove scope changes clear history before returning to the earlier bank, and disclosure withdrawal clears retained bodies. Intercepted APIs, separate from actual Run learning.                                                    |
| Generated API, TypeScript, Vite     | Passed                                                     | `/tmp/aidash-125-api-generation-7.log`, `/tmp/aidash-125-types-26.log`, `/tmp/aidash-125-vite-15.log`; current production DTO/UI.                                                                                                                                                                                   |
| Formatting, ESLint, Ruff            | Passed                                                     | `/tmp/aidash-125-trunk-3.log`; `trunk check --ci --no-fix --filter prettier,ruff,rustfmt,taplo,eslint`. Clippy is verified separately.                                                                                                                                                                              |
| Dependency ownership                | Passed                                                     | `/tmp/aidash-125-boundaries-5.log`; full locked Cargo metadata with `scripts/check-workspace-boundaries.py`.                                                                                                                                                                                                        |
| Frozen English/Japanese evaluation  | Extraction label recall 1.0; useful recall 0.75 per locale | `/tmp/aidash-125-kubernetes-evaluation-8.log`; `.ignore/transaction-acceptance/aidash-tx-b061d66b1d/`. Zero unsupported/unlabeled/duplicate/false-success/stale-after-correction results. Seven charged calls/eight reported tokens per locale, synthetic zero-price policy; production quality remains unapproved. |

## Recovery profile

Typed CBOR archives and an independent fsynced ledger retain epoch,
revision/body floors and permanent deletion identities. Restore closes its
persistent gate before database locks. SIGKILL leaves reopened serving
unavailable until reconciliation. Writes and occurrence windows truncate to
canonical microseconds before persistence/digest observation, avoiding Linux
PostgreSQL rounding differences.

The profile restores memory bodies into a live Home while preserving current
authority, Registry pins, source journals, bank identity, writer origins, budget
reservations, operation receipts and deletion state. Old/expired bodies and
revoked/removed evidence remain withheld before reindexing and disclosure.
Full PostgreSQL rewind and HA/failover require independent continuity acceptance.
Legacy JSON backups are unsupported.

## Cluster and browser acceptance

k3s full native acceptance passed with cleanup completed:
`/tmp/aidash-125-k3s-native-3.log`, evidence
`.ignore/transaction-acceptance/aidash-tx-055919934a/`. Four ordinary/generated
en-US/ja-JP remote cases, both frozen evaluation cases and four canonical Run
learning/review cases passed. Backend image index:
`dbdea84d047ee371e37f6bf602b21d75c89895e4f4842a02d7e946f45a95fc26`;
source fingerprint:
`b309e4493516a1686fb85087639dbb156f9368145e39b7b0aac2087e488f36b2`.
Later changes format migration files and the TOML fixture; no runtime behavior
changed.

Final Kubernetes all-UI acceptance passed and cleanup completed:
`/tmp/aidash-125-kubernetes-native-14.log`, evidence
`.ignore/transaction-acceptance/aidash-tx-43fc9ac6a6/`. Four ordinary/generated
en-US/ja-JP remote cases, both frozen evaluation cases and four real-browser
canonical Run candidate reviews passed. Backend image index:
`b3e26e7b8a2077ef77b1e16bda50d959f924453271e8436db7394565d27b74fd`;
PostgreSQL image index:
`bd062dd62400400d4ee0e916528d8f8ef038ec5c9469af2f203fdf780868b7b0`;
source fingerprint:
`107f567b1374eee86fdf1c88b197be439f589634b36dcce2240cac3d7279d666`.
Runtime, driver and UI source hashes match the launch manifest. Each browser
result additionally records the actual built frontend/test/config file hashes
and a screenshot. Later verification-document updates change no runtime input.
Both runners exited zero; both disposable namespaces/clusters were removed.
The user's default Kubernetes context remains `orbstack`.

Actual synthetic-fixture browser screenshots are preserved in
[English](screenshots/native-memory-en-US.png) and
[Japanese](screenshots/native-memory-ja-JP.png). These capture the immediate
post-review screen while query refresh may still be in progress. The recorded
API responses and subsequent index-ready assertions establish completed
admission independently of the screenshots.

The browser profile uses an authentication-presentation shim with an actual
subject bearer. State, Registry and memory endpoints reach the live Home; no
memory endpoint is fulfilled by a fixture. This proves real human admission,
not OIDC login. Two pending candidates retain generated authority during review,
indexing and history. Rejecting the remaining candidate permits retirement
under the original lifetime/allowance. Current authority then denies history;
replay does not extend authority.

Evidence directories retain dirty base SHA/source hashes, image manifests,
launched driver hashes, raw API/provider results and content-free ledger status.
Compare runtime hashes and driver hashes independently after test-only edits.
Recent failed attempts are not green evidence: learning-11 found JSONB-null
rejection replay (fixed; review-replay-50 passes), and k3s native-2 was interrupted
when parallel setup replaced a running signed `kubectl`. Tool/diagnostic paths
are now isolated per run. Learning-10 reached the real dialog but failed on its
unnamed-dialog selector; corrected. Learning-12's four browser cases passed,
but the shell wrapper exited 127 after an in-place script edit during execution;
it is not a passing full runner. The launch script was restored before k3s-3
finished (its hash matches the launch manifest), and k3s-3 exited zero.
Subsequent runtime, runner and UI edits were frozen during final Kubernetes
acceptance.
Native-47's restore startup deadline failed
under concurrent builds; focused SIGKILL and full native-49 reruns passed without
extending that deadline.

## Reproduction

Use Rust 1.96, `RUSTC_WRAPPER=''` and a declared task target directory. Database
suites use `RUST_MIN_STACK=8388608`, `RUST_TEST_THREADS=1` and an isolated test
peer credential. Fixtures apply the native migration graph in disposable DBs.

```sh
cargo test -p aidash-domain -p aidash-application -p aidash-harness --lib
cargo test -p aidash-server --test reinhardt_persistence
cargo test -p aidash-server --test scoped_remote_execution
cargo test -p aidash-server --test generation_semantic
cargo clippy -p aidash-server --all-targets -- -D warnings
cargo metadata --locked --format-version 1 | python3 scripts/check-workspace-boundaries.py
scripts/generate-api.sh
```

From `web/`, run `npx tsc -b`, the Vite build and
`npx playwright test --config playwright.ui.config.ts native-memory.spec.ts collaboration.spec.ts`.
Actual disposable-cluster profiles:

```sh
scripts/test-cluster.sh kubernetes native-memory
scripts/test-cluster.sh k3s native-memory
scripts/test-cluster.sh kubernetes native-memory learning-ui
scripts/test-cluster.sh kubernetes native-memory all-ui
```

Native partitions: `remote`, `evaluation`, `learning`, `learning-ui`, `all-ui`. Default
runs all backend cases; `learning-ui` additionally needs installed dependencies,
a built frontend and Chromium. `all-ui` runs every backend case with real browser
human review. Temporary explicit kubeconfigs preserve the
current context; evidence survives namespace/cluster cleanup.

## Release inputs

Production model/embedding/reranker/tokenizer versions, prices and English/
Japanese quality approval remain release inputs. Issue #73 service/retention/
RPO/RTO targets and Issue #122 production quality/cost thresholds are unapproved.
Synthetic zero-price fixtures do not approve them. No live user database was
reset by this task; native schema application discards legacy JSON banks.
