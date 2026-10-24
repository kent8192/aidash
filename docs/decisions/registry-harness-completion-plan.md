# Registry Harness Completion Implementation Plan

> **For agentic workers:** Use isolated subagent-driven tasks, followed by integration and independent review. The owner approved implementation and one PR on 2026-10-23; no further plan approval is required.

**Goal:** Complete #108–#110 and verify the already implemented #331 in one PR.

**Architecture:** Pure declarations and invariants stay in domain; shared selection/observation use cases and ports stay in application; Harness operates the generic source port; server owns durable adapters, approval lifecycle, RLS and HTTP. Existing provider routes and memory/research safety remain authoritative.

**Tech stack:** Rust 2024, Reinhardt ORM/query/migrations, PostgreSQL, existing decision providers, React/TypeScript and Playwright.

**Spec:** [Approved design](registry-harness-completion.md), ADR 0084–0086 and existing ADR 0083.

## Global constraints

- One PR; no rebase or force push; preserve unrelated worktrees.
- Undeployed schema cutover; recreate incompatible development data, never rewrite immutable records.
- Declaration is not authority; live checks, exact pins, durable charges and required-Home receipts remain mandatory.
- Shadow does not settle or delay human approval; labels originate only in explicit human decision transactions.
- Default source request bytes remain stable; Scored and new query policies are opt-in.
- No paid calls or private uploads in ordinary checks; human calibration cannot be inferred from fixture agreement.
- PR #481 is already merged into #478. Start from that integration and initially target #478's branch if it is still open; retarget after its merge.

## Review focus

- A human can answer before the observer; the late result must not settle or relabel the action.
- A worker loses its lease after a paid attempt; recovery must not dispatch or charge twice.
- A source changes or is revoked after retrieval; no stale adoption or false delivered/full-read receipt.
- Fitting removes content selected by the Decider; only the final accepted inference earns delivery.
- Required-Home and generated-chain rules must not be bypassed by defaulting, scoring or unavailable fallback.

## Task 1: Built-in compaction default

Owner: isolated `compaction` worktree. Files: domain Registry normalization/defaults, application Registry system seed, related tests and necessary server approval/compaction admission integration.

- [x] Extend normalization tests for ordinary default, explicit removal/replacement and generated exception.
- [x] Add exact built-in compaction definition with ADR 0042/0066 Routes and tenant materialization.
- [x] Preserve explicit live Catalog/credential checks and typed pauses.
- [x] Verify domain/application focused tests; commit the coherent task.

## Task 2: Source contract and selection

Owner: isolated `source-contract` worktree. Files: domain context-source and Registry source declarations; application ports/use cases; related tests. Communicate exact public interfaces to Task 3 before its integration.

- [x] Define bounded candidate/contribution/outcome, exact operation edges, pin/volatility/query policies and stage receipts.
- [x] Implement stable authorized candidate selection, Noul evidence, Shadow/Enforce and typed failures.
- [x] Preserve one primary memory writer and reject mandatory-Home Enforce filtering.
- [x] Test narrowing, stable selection, default rendering contract, revocation and fetched/selected versus accepted delivery; commit.

## Task 3: Generic native source execution

Owner: isolated `source-runtime` worktree. Files: Harness source collection/request fitting and server source/provider adapters; execution ports/tests as needed. Consumes Task 2's interfaces; preserve its implementation via merge before final verification.

- [x] Replace individual documents/skill/semantic assembly entry points with the common source port and native providers.
- [x] Preserve Legacy/Ordered/Native default request bytes and existing retrieval/recovery policies.
- [x] Add opt-in accepted-inputs-first query policy, bounded detail, provenance/dependency-safe dedup and final accepted-delivery recording.
- [x] Exercise remote/required-Home, empty refresh, clipped detail, fitting and recovery; commit.

## Task 4: Shadow observation and report contracts

Owner: isolated `observation-contract` worktree. Files: new domain decision observation/report modules, application observation use cases/ports and tests. Communicate exact APIs and persistence requirements to Task 5 and UI.

- [x] Define typed action frames for all five kinds, observation identities/statuses, human labels and bounded projections.
- [x] Implement portable observer dispatch/recovery and exact-version restrictions without approval settlement.
- [x] Implement grouped 2-by-2 reports, maximum-category risk bands, exclusions and zero-denominator N/A.
- [x] Verify recovery/idempotency, unavailable outcomes and label/denominator distinctions; commit.

## Task 5: Native independent observation

Owner: isolated `observation-server` worktree. Files: server models/migrations/RLS, observer journal/worker, capability and Generation approval creation/explicit decisions, tenant settings, HTTP/report projections, native tests. Consumes Task 4's contracts.

- [x] Persist immutable frames and observer-owned leases; enqueue independently of approval visibility and settlement.
- [x] Record explicit human labels atomically in capability decide and Generation Approve/Deny transactions.
- [x] Add tenant opt-in exact Shadow version setting, permission-scoped per-approval projection and aggregate API.
- [x] Verify PostgreSQL races/recovery, no fabricated Generation action, tenant isolation, quotas and disabled-zero-call behavior; commit.

## Task 6: Dashboard and editors

Owner: isolated `dashboard` worktree. Files: web UI/editor, localization and meaningful UI/unit tests only. Consume communicated DTOs; API generation is consolidated at integration.

- [x] Add observer configuration, per-approval comparisons and Tenant aggregate report with explicit no-data/missing states.
- [x] Expose source policies/pinning and explicit compaction removal/replacement with exact approval requirements.
- [x] Preserve current authorization and accessible UI conventions; English/Japanese labels; no state/arguments leakage.
- [x] Verify TypeScript/unit/browser behavior; commit.

## Task 7: Integration, review and delivery

Owner: root. Merge each committed task normally; resolve semantic overlaps in contracts once. Maintain ignored notes for task SHAs, commands and findings.

- [x] Reconcile docs/Issue acceptance evidence, prior PR #290 review closure and #331 regression evidence.
- [x] Run pure crates, workspace boundaries, native PostgreSQL/migrations/HTTP, frontend and browser checks proportional to changed behavior.
- [x] Obtain independent spec/quality review, fix actionable findings and repeat only affected checks.
- [x] Commit and normal-push the remaining work through one linked PR, with live labels, attribution and exact validation evidence; preserve the canonical parent integration after the owner merges it.
- [x] Verify local/upstream/remote/PR-head parity, labels and hosted CI separately from local and live acceptance.

The owner merged PR #500 into #478 at `036358fb218b6c28a050c41cd690e50fe6159a09` while final verification was in progress. The remaining test corrections and verification record are consolidated in that existing integration; the original parent worktree is preserved. PR #500 uses `enhancement` and `breaking-change`; the canonical administration repair #478 retains `bug` and `breaking-change`.

Local verification on 2026-10-24 passed the 6,326-case pure-crate checkpoint (one existing ignored case), followed by 170 affected checks after the Research dependency update. Compaction checks passed the 14 originally discoverable native cases and all four affected cases after correcting test parameterization. Native checks passed 16 Source cases, 25 Provider Credential cases, nine Research retention/replay cases, seven core approval/context/revocation cases, peer preflight and 11 server-library cases. All 39 migration cases were verified: 36 passed in the full run; three retained their 35-second deadlines and passed with SHA256-identical CLI binaries on the internal volume after an external-volume loader stall was observed. The real HTTP run passed 1,275 requests across 338 endpoints. Dashboard unit/browser checks, the approval-card regression, generated API/SDK, TypeScript, production build and architecture checks passed.

OnDemand controls automatic payload reads and retains the required primary-memory Run authority. Its native regression independently makes the primary payload loader fail, then verifies shared-only inference with no primary delivery or full-read receipt. Runtime verification precedes the final behavior-preserving lint cleanups. Strict Server Clippy (`--locked --all-targets --all-features -- -D warnings`) passed after those cleanups, on the exact integrated source tree. Hosted CI and paid-provider/human-calibration evidence remain separate from these local results.
