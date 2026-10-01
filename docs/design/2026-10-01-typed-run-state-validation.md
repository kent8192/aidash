# Typed Run state implementation and validation

Source: [Issue #62](https://github.com/kent8192/aidash/issues/62), the accepted [specification](2026-10-01-typed-run-state-specification.md) and [decision interview](2026-10-01-typed-run-state.md).
Branch: `feat/issue-62-typed-run-state`, based on `develop/0.1.0` at `3cb803544c37ebbeb413bc8554accb0a06d916fa`.
Status: implementation and local verification complete, 2026-10-01.

## Implemented behavior

- [Run state](../../src/domain/run_state.rs) owns phase-specific execution data and derives `RunPhase`. Waiting reasons and legal continuations, control/actions, Task status, response progress, approvals, read plans, recovery metadata and [Context](../../src/context.rs) history/usage use typed Rust values.
- The private storage codec requires `state_version: 1` and rejects unknown machine fields, unsupported variants/versions and incomplete required envelopes. Arbitrary tool arguments/results and human response content stay JSON; required content may contain `null` but may not be absent. Existing physical TEXT/JSONB columns remain.
- [Raw-row inspection](../../src/domain/run_state/row.rs) keeps malformed Runs visible without fabricating executable Context. Authorized inspection, pause and cancellation work; resume rejects unsupported state. The dashboard displays the state error and disables resume.
- [Recovery and failure delivery](../../src/store/run_state.rs) use typed scheduling with database time, bounded keyset pages and shared notification/recovery eligibility. Invalid eligible Runs receive fenced failure-delivery intents and durable events. Live leases, pause, terminal outcomes, cancellation, input/effect journals and execution authority remain enforced. Failure/Home notification and terminal input delivery use narrow metadata even when Context is invalid.
- [The forward migration](../../migration/src/m20261001_000000_typed_run_state.rs) updates canonical defaults, approval/request references and JSON-dependent triggers. It performs no legacy conversion. Tool invocation preparation persists its derived phase and payload together with the invocation journal entry.
- API/Federation inspection and commands, OpenAPI and the web client use the current format. Generated contracts and frontend build output retain the existing ignore rules.

## Local checks

Rust test scripts provide their immutable local fixture credentials; no runtime user configuration was changed. On this macOS host, commands use `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, `RUSTC_WRAPPER=` and the existing shared Cargo target directory. The full Rust gate uses `RUST_TEST_THREADS=4`.

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` and `git diff --check` | Passed. |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Passed with no warnings on the final implementation. |
| `cargo test --locked --test migrations --test record_constraints --test postgres` | 61 passed: 2 migration, 29 constraint and 30 PostgreSQL cases. Includes the final SeaQuery-generated column and foreign-key DDL. |
| `scripts/test-rust.sh` | Passed: 670 tests across 52 test binaries; 0 failures. Two default ignores: the separate SSE performance benchmark and the embedded Worker subprocess helper. Exit code 0. |
| `scripts/test-worker-activation.sh` | 19 passed; one subprocess helper is intentionally ignored in the parent test invocation. Exit code 0. Includes separate worker process, broker faults/outage, restart, lease expiry, invalid-state isolation and terminal input delivery evidence. |
| `npm run build --prefix web` | Passed, including OpenAPI export, client generation, TypeScript and the production bundle. |
| Generated schema inspection | Passed: seven Run phases with the retained values, seven RunState variants, eight waiting variants and the existing eight Task statuses. |
| Web graph/collaboration unit tests | 92 passed: 55 graph and 37 collaboration cases. |
| ESLint for the changed web files | Passed. |
| Relevant Playwright browser journeys | 13 passed, including local/remote controls, mobile layouts, human responses and inspection/cancellation with resume disabled for invalid state. Uses the repository's deterministic UI fixtures. |
| PR screenshot capture | 2 passed using the same UI fixtures: [normal active execution](2026-10-01-typed-run-state-valid-run.png) and [invalid paused execution](2026-10-01-typed-run-state-invalid-run.png). The invalid-state panel shows its error, disabled Resume and enabled Cancel. |
| Local design/specification/validation links | Passed. |

The final checks use the execution-source and generated-contract snapshot in `target/typed-run-state-verification/source.json`. The final source matches that snapshot; only documentation was updated after the checks began. Check logs and `results.json` are retained in the same directory. Dedicated worker evidence is retained under `target/activation-evidence-issue62-delivery`, including its invocation record, source patch and fault/outage/process observations. These are local generated artifacts under the existing ignore rules.

The acceptance cases exercise handwritten format fixtures, malformed-state isolation ahead of healthy work, 129 future waits before a later runnable Run, authorized inspection/stop controls, invalid-Context Home retries across an outage and restart, terminal input drainage, and notification execution in a separate worker process. Existing approval, uncertain-effect, read-plan, media, input, authorization and transaction fixtures retain their behavioral assertions under the current format.

The primary new acceptance assertions live in [codec fixtures](../../src/domain/run_state/tests.rs), [database isolation and scheduling](../../tests/postgres/typed_state.rs), [normal notification execution](../../tests/worker_activation/review.rs), [authorized inspection/control](../../tests/execution_authorization.rs) and [browser inspection/control](../../web/tests/collaboration.spec.ts). [Home retry](../../tests/postgres.rs), [terminal input delivery](../../tests/run_message_finalization.rs) and [invocation projection recovery](../../tests/observation.rs) extend their existing owning fixtures rather than introducing parallel mock pipelines.

## Operational boundary

This is the accepted breaking state/DTO format. Old-format Runs are not converted into resumable current-format state. Use the specification's coordinated server/worker/web/peer upgrade and fenced disposition rules; SQL migration rollback does not make current-format execution data suitable for an older binary.

These are local results for the execution-source snapshot above. Hosted CI is evaluated separately on the published PR head.
