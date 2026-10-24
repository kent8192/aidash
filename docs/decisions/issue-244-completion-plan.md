# Issue 244 Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete all remaining descendants of Issue 244 in one pull request.

**Architecture:** Extend the existing Tenant/Node boundary with a recoverable
Node lifecycle and explicit ownership. Put a time-selected keyring above the
existing File/KMS signers, and advance receiving trust independently of
authorization. Keep billing admission local; integrate only the payment model
proven by the Stripe sandbox experiment.

**Tech Stack:** Rust 2024, Reinhardt ORM/Query/migrations, PostgreSQL RLS, existing
File and Cloud KMS signing adapters, React/TypeScript, Playwright, Stripe Managed
Payments and the official Stripe Terraform provider.

**Spec:** [Issue 244 completion design](issue-244-completion.md), ADRs
[0083](../adr/0083-node-deletion-is-recoverable-and-releases-capacity-before-purge.md),
[0084](../adr/0084-normal-deployment-key-rollover-is-proven-by-the-currently-trusted-key.md),
and [0085](../adr/0085-live-preview-pods-use-the-execution-pod-time-meter-until-they-stop.md).

## Global constraints

- One branch and one final PR: `feat/issue-244-complete-sub-issues` targeting
  `develop/0.1.0`. Preserve unrelated work and never rebase or force-push.
- Start from `37d427e09b519fbede493d01997e2447472126dd`; merge develop forward
  when needed. Earlier implementation branches are already merged.
- Node grace: default 30 days, allowed 1–90 days, fixed at deletion. Only live
  Nodes count. Restore refuses when current capacity is insufficient. Never
  delete the last live Node or the current default Node.
- Immediate Node purge is Deployment Administrator only. Retained Node files
  remain billable until physical purge. Tenant-deletion exemptions still apply.
- Normal key update: publish, activate after 24 hours, retire predecessor seven
  days after activation; one transition at a time, one bounded proof per request.
- Keep the existing 8192-byte Federation token limit, including an embedded
  transition proof. Preserve method/path/query/body binding and token lifetimes.
- Tenant-granular RLS remains. Classify every new table and every Node purge
  selector. Keep finite audit retention, default 365 days, and name tombstones.
- Domain/application/harness dependency directions, `module.rs` layout,
  Reinhardt persistence and raw-DDL exception rules remain mandatory.
- Databases are recreated; do not add legacy compatibility or guess ownership
  for old service subjects. Agent/service subjects require a Node; users do not.
- No synchronous Stripe call in admission or while holding account/capacity
  locks. CI uses fakes. Actual Stripe sandbox acceptance remains mandatory.
- Build only the sandbox-proven billing model. No inference resale, launch
  prices, Aidash billing email or gateway-traffic pricing is introduced here.
- Commit messages use Conventional Commits and end with this agent's model
  attribution. The final PR links the Issues, uses live labels and Codex
  attribution, and remains draft until delivery acceptance is complete.

## Review focus

1. A cached credential, existing Worker lease or same-process Federation request
   must not bypass a Node deletion; Task 3 owns these tests.
2. Purge must preserve sibling Nodes, shared Tenant policy and shared files;
   Task 4 owns coverage and retry/race tests.
3. A valid transition proof with an invalid outer request must not mutate trust;
   Task 7 owns this and quarantine/update races.
4. Attachment moves and month-long previews must bill the correct account and
   period exactly once; Task 11 owns interval/export tests.
5. A pending downgrade or Stripe outage must not make Node creation wait on
   Stripe; Task 10 owns concurrency and local-admission tests.

## Execution order and ownership

Task 0 is the first billing task. If credentials are absent, mark it blocked and
continue independent Node and key work; do not infer its outcome. Tasks 1–5 form
the Node lane, Tasks 6–8 the key lane, and Tasks 9–12 the billing lane. Tasks 9,
10 and 12 require Task 0's result. Task 11's metering corrections may proceed
before that result. Task 13 integrates all lanes.

Use isolated worker checkouts when delegating edits, one writer per checkout.
The coordinator owns migration numbering, shared bootstrap/configuration files,
generated API contracts and integration commits. Agree the interfaces below
before parallel implementation; integrate normal commits into the one branch.

Existing PR 478 owns the Tenant restore/Plan Connection retirement repair. Track
its head, owner and hosted acceptance; inherit it after integration into develop
rather than racing its author or independently reproducing it. Its unresolved
delivery is an explicit final acceptance dependency.

Issue 405 owns the live preview runtime. PR 461 supplies host-bound Secrets and
sandbox egress; it does not implement a preview that survives Run completion.
Task 11 may verify the existing generic usage consumer independently, but actual
preview start/Run completion/stop acceptance depends on Issue 405. Do not invent
a second preview runtime here or report a synthetic meter contract as preview
end-to-end evidence.

Each task adds its primary behavioral regression at the strongest owning
boundary, demonstrates the intended pre-fix failure, implements the change,
runs its focused verification and commits. Do not duplicate a contract at every
layer, add tests that grep implementation text, or export test-only seams.

## Task 0: Prove the Stripe model in the actual sandbox

**Files:** Create `scripts/stripe-sandbox-spike.py` and
`docs/operations/stripe-billing.md`; put local result metadata under `notes/`.

**Interface:** The spike emits JSON with `winner`, `api_version`, `request_id`,
`object_ids` and `compatibility_error`. `winner` is `metered_overage`,
`usage_credits` or `inconclusive`; an authentication/network failure is always
inconclusive. It never emits a key, authorization header or customer personal data.

- [ ] Read an explicitly supplied sandbox credential file without shell sourcing
      or echoing it. Refuse live keys; use an explicit API version.
- [ ] Create a disposable eligible Product, Billing Meter and metered Price,
      then actually create and complete Managed Payments Checkout using that Price.
      Exercise the metered usage through a supported sandbox billing-cycle control
      and verify the resulting quantity and charge. Record Stripe request/object IDs,
      subscription state and billing evidence; Session creation alone is insufficient.
- [ ] If Stripe gives a definitive compatibility refusal, test Managed Payments
      one-off Checkout for Usage Credits. Do not treat an outage as a model decision.
- [ ] Record the result in Issue 242 and the operational evidence, remove only
      owned disposable resources when safe, and commit the runner and instructions.

Run: `python3 scripts/stripe-sandbox-spike.py --output notes/stripe-spike.json`.
Pass means an actual sandbox result and the matching selected integration, not
just a successful fake or an uncompleted Checkout Session.

## Task 1: Node lifecycle, ownership and persistence contracts

**Files:** Create `crates/aidash-domain/src/nodes/deletion.rs`,
`server/src/apps/identity/models/node_deletions.rs` and
`server/src/apps/identity/repositories/node_deletions.rs`. Modify
`crates/aidash-domain/src/{nodes.rs,policy.rs,identity/tenancy.rs}`,
`server/src/apps/identity/repositories/nodes.rs`, app model/module registries,
`server/settings/base.example.toml`, migrations and database classification.

**Interfaces:** Add `Subject.node: Option<NodeName>` with validation: agent/service
must name a Node, user must not. Add `TenancyRules.node_deletion_grace_days`.
Repository functions `nodes::{get_live,list_live,count_live}` take the current
Tenant transaction and Tenant name; `get_live` also takes a Node name and returns
`Result<Node>`, `list_live` returns `Result<Vec<Node>>`, `count_live` returns
`Result<u32>`. `node_deletions::current_locked(tx, tenant, node)` returns
`Result<Option<NodeDeletion>>`.

`NodeDeletion` records deletion identity/time, immutable `purge_after`, restore
and purge timestamps, retry step/failures and worker lease. Keep historical
deletions; do not overwrite a prior deletion on delete/restore/delete.

- [ ] Extend domain tests with 1/30/90-day acceptance, 0/91-day refusal,
      exact deadline behavior and invalid subject ownership.
- [ ] Add ORM models and generated migration state, required constraints,
      Tenant RLS and classification. Update fixtures with explicit subject ownership.
- [ ] Verify domain tests, startup/classification and migration apply/rollback;
      commit `feat(identity)!: model recoverable Node deletion and subject ownership`.

Run: `cargo test --locked -p aidash-domain --lib`; then server `startup` and
`migrations` targets. New ownership must not add an application/domain dependency
on server or Reinhardt.

Task 1 local evidence: domain 2,187, application 3,755 and startup/classification
15 tests passed. The deletion-deadline regression failed across a daylight-saving
transition before the CHECK used elapsed-hour bounds. Both focused in-process
migration tests then passed on the final source, including rollback/reapply,
model/schema agreement and the ownership, RLS and immutable-deadline constraints.
The independent fix review approved the integrated range ending `94b2c4cde2`.
The separate external migration CLI fixture stalled locally before SQL; broad
migration and hosted acceptance remain Task 13 requirements.

## Task 2: Default selection, delete/restore and live capacity

**Files:** Modify identity `services/nodes.rs`, `views/nodes.rs`,
`serializers/nodes.rs`, `urls.rs`, `serializers/openapi.rs`,
`crates/aidash-domain/src/identity/tenant_administration.rs`, and Enterprise
`billing/nodes.rs` and capacity consumers.

**Interfaces:** Add `node.delete` to policy actions and administrator defaults.
Use `node.update` for `POST /api/tenants/{tenant}/nodes/{node}/default`;
`node.delete` for sibling `POST .../deletion` with `{confirmation: node_name}`
and `POST .../restore`; reserve `POST .../purge` for Deployment Administrators.
All lifecycle/capacity changes share the existing `nodes:<tenant>` lock.

- [ ] Extend `dashboard_oidc/tenancy/nodes.rs` to prove default/last-live deletion
      refusal, wrong confirmation refusal, immediate slot release, restore conflict
      at the current limit and current-policy authorization after intervening edits.
- [ ] Implement routes and administration history with live counts for creation,
      restoration, Billing Account attachment and Tier change. Never silently choose
      another default during deletion or evict a live Node during restoration.
- [ ] Verify the HTTP tests plus Enterprise billing delete/downgrade/restore
      concurrency; commit `feat(identity)!: manage Node deletion and restoration`.

Run: `cargo test --locked -p aidash-server --test dashboard_oidc tenancy::nodes`;
Enterprise `--test billing` with the same package/manifest command as Task 10.

Task 2 local source evidence: independent review approved the lifecycle API,
explicit Node administration audit and live-capacity changes. Node HTTP tests
10/10, migration tests 3/3, the real concurrent delete/restore and Tier-change
capacity test, and strict server library clippy passed. The broader Tenancy run
was 70/73; its three purge-receipt failures passed in the unchanged serial owner
run (12/12), without establishing the parallel failure's cause. Task 3 cancellation,
Task 4 physical purge/measured bytes and Task 5 UI remain separate acceptance.

## Task 3: Stop deleted Node work at every real boundary

**Files:** Modify identity `services/access.rs`, `services/identity.rs`,
`services/execution.rs`, `repositories/execution.rs`; application
`ports/federation/inbound.rs` and `federation/inbound.rs`; server Federation
`remote/services/inbound.rs` and `remote/repositories/routes.rs`; execution
stop/cancellation owners where the existing Run lifecycle requires them.

**Interface:** Add the Node-live query to the existing inbound repository port,
alongside `tenant_live`. Require a live Node in `node_resource`, `use_node`,
`workspace_node`, `catalog_node`, Subject identity locking, outbound sending-node
selection and Worker lease inheritance/refresh. A `node_names` tombstone is not
evidence of a live Node.

- [ ] Add owner-boundary cases for browser/Workspace-ID access, Subject
      Credentials, an existing Worker lease, HTTP and same-process Federation, and
      active streaming. The deleted Node is refused; a sibling Node still works.
- [ ] Connect deletion to cancellation of nonterminal Runs and queued work,
      including delegated work, using existing cancellation and revocation paths.
      Record the stop contract for Issue 405's future hosted previews. Preserve
      decided atomic commits and existing recovery rules.
- [ ] Verify restoration never restarts canceled work and cannot restore an
      independently revoked credential; commit `fix(execution): enforce Node lifecycle boundaries`.

Run server targets `execution_authorization`, `worker_activation` and the full
`scoped_remote_execution`, then the identity/execution/Federation partitions.

## Task 4: Resumable Node purge with explicit ownership coverage

**Files:** Create identity `services/node_purge.rs` and
`repositories/node_purge.rs`. Extend `server/src/database/classification.rs`,
reuse `database/purge.rs` dependency ordering, and wire the existing maintenance
loop in `server/src/bootstrap.rs`. Add
`identity/tests/dashboard_oidc/tenancy/nodes/purge.rs`.

**Interface:** `node_purge::sweep(store, audit_retention_days) -> Result<()>` finds
work administratively, then performs each destructive step in its owning Tenant
scope with a fenced lease. Keep explicit Node-owned, shared and retained
selectors; deriving a selector from a guessed `node` column is forbidden.

- [ ] Build the primary purge fixture with two Nodes and another Tenant,
      actual object files, subjects, credentials, approvals, mappings and audit.
      Assert exact target deletion and preservation of every shared/sibling owner.
- [ ] Delete owned object files before rows, handle references such as
      `core_objects.area_id -> core_areas.workspace_id -> workspaces.node`, and
      classify records without that relationship explicitly. Preserve shared policy
      revisions and foreign receivers' registration ownership.
- [ ] Implement idempotent retry, restore/purge exclusion, Tenant/Node purge
      exclusion, deadline expiry, stalled status and audit expiry. Preserve
      `node_names` permanently; verify the coverage guard fails on an omitted
      Node-owned classification.
- [ ] Run the purge HTTP tests and migration/classification tests; commit
      `feat(identity): purge Node-owned data without deleting shared Tenant state`.

## Task 5: Node lifecycle dashboard and generated contracts

**Files:** Modify `web/src/{nodes-settings.tsx,nodes.tsx,node-selection.ts}`,
translations, `web/tests/nodes.spec.ts` and selection unit tests. Generate the API
client and Bruno catalog from their existing scripts.

- [ ] Extend the browser spec with default switch, confirmed deletion, selection
      fallback, deleted-Node management, deadline, retained storage costs, restore
      conflict and purge progress. Verify the last live Node has no delete action.
- [ ] Implement the UI using real API state; ordinary selectors show live Nodes,
      while the authorized management page also shows restorable/deleting Nodes.
      Keep physical file retention and its ongoing storage charge visible.
- [ ] Run API generation, TypeScript/build and the Node Playwright spec against
      the owning HTTP fixture; commit `feat(web): manage deleted Nodes and recovery`.

Run `scripts/generate-api.sh`, `python3 scripts/bruno_contracts.py --check`,
`npm run build --prefix web`, then the existing Node UI/browser project.

## Task 6: Bounded rollover proof and trust state

**Files:** Create `crates/aidash-domain/src/nodes/keys.rs` and
`crates/aidash-capability/src/federation/rollover.rs`. Modify their module
declarations, domain Node Registration types and capability Federation claims.

**Interfaces:** Domain types `PublicDeploymentKey`, `RolloverSchedule`,
`TrustedDeploymentKeys` and `RetiringDeploymentKey` carry IDs/public keys,
publication/activation/retirement times and quarantine state. Keep capability a
leaf crate, using wire strings rather than a new dependency on domain.
`mint_transition(claims: &RolloverClaims, predecessor: &dyn TokenSigner)` is async
and returns `Result<String, SignError>`; `verify_transition(compact, trusted,
deployment, now)` returns `Result<RolloverClaims, FederationFailure>`.

- [x] Add contract tests for the exact 24-hour/7-day boundaries, forged proof,
      wrong Deployment, successor key digest mismatch, future activation, retirement,
      replayed predecessor and oversized token. Existing request-binding tests remain.
- [x] Add one optional compact proof to Federation claims within the total
      8192-byte limit. Verify both transition and successor-signed outer token,
      including the original request binding; a proof alone never admits a request.
- [x] Run capability/domain tests and commit
      `feat(federation): authenticate bounded Deployment Key transitions`.

Task 6 local evidence: 2,228 capability/domain tests and strict clippy passed;
independent specification and quality review approved. The integrated commits
are `1d45dfedf4` and the consumer initialization `4696173d7b`. Receiving
persistence and deployed sender continuity remain Tasks 7 and 8.

## Task 7: Persistent receiving keysets and automatic advancement

**Files:** Modify Federation `peer/models/node_registrations.rs` and history,
identity `models/authorization_peer_graph_grants.rs`, migrations/classification,
application `federation/{inbound,registrations}.rs` and inbound ports, server
`remote/services/inbound.rs`, identity `repositories/node_registrations.rs` and
`repositories/graph/persistence.rs`.

**Interface:** Add keysets and `keys_revision` separately from authorization
`revision`. `advance_keys(owner, expected_keys_revision,
expected_authorization_revision, keys) -> Result<bool>` performs a compare-and-set
for a particular receiving registration/grant under its existing authority.

- [ ] Extend receiving HTTP/PostgreSQL tests: accept the successor with its proof
      while the public endpoint is down, persist it, restart and keep accepting the
      successor after overlap. An unlearned receiver refuses an expired transition.
- [ ] Persist advancement only after the outer request verifies and current
      Node/registration/grant guards still hold. Test a valid proof with an invalid
      request causes no change, plus concurrent disable/quarantine and duplicate
      advancement. Never overwrite a newer trust state.
- [ ] Verify Graph cursors survive a crypto-only change because authorization
      revision, enabled state and permissions remain unchanged; commit
      `feat(federation): advance receiving trust without changing authority`.

Run `dashboard_oidc tenancy::registrations`, `peer_graph_review`,
`peer_graph_nodes`, migrations and startup/classification tests.

## Task 8: File/KMS keyring, operator controls and continuity

**Files:** Extend application `federation/key.rs`, integrations
`federation.rs` and `federation/key.rs`, Enterprise KMS Deployment Key adapter,
server `peer/services/key_commands.rs`, runtime/bootstrap and public identity
serializers/services. Create `peer/repositories/deployment_keys.rs` and a
Deployment-wide `deployment_key_rollovers` model/migration. Extend registration
and Graph services/views/serializers plus the Node Registration dashboard.

**Interfaces:** Keep the existing raw `DeploymentKey` signer unchanged.
`DeploymentKeys::signing(now: u64) -> Result<FederationSigning>` returns an
`Arc<dyn DeploymentKey>` and optional proof;
`DeploymentKeys::publication(now: u64) -> Result<DeploymentKeyPublication>` returns
public state. Construct the ring after the pool exists. Store source references,
public proof and one authoritative schedule in the DB; never private PEM.

- [ ] Extend File/KMS adapter and CLI tests to select the new signer at activation,
      reject concurrent normal rollovers and missing sources, agree across restarted
      processes, and publish the current keys without listing Nodes or secrets.
- [ ] Add `deployment-key rollover --file <path>` or `--kms <version>` and
      `status`; keep initial `generate` DB-independent. After activation, an unavailable
      new signer fails rather than falling back to the predecessor.
- [ ] Add quarantine and independent-fingerprint confirmation under existing
      record management rights. Normal proof cannot clear quarantine; confirmation
      cannot enable a disabled registration/grant. Bound bulk actions to records the
      actor may administer. Verify wrong-Tenant/member denials through HTTP.
- [ ] Extend the existing two-Deployment fixtures with a running Run and atomic
      transaction across activation, proving continuity and no repeated effects.
      Update protocol/operations docs; commit
      `feat(federation): operate scheduled File and KMS key rollover`.

Run capability/integrations/GCP signer tests, server key-command tests,
`transaction_protocol`, `transaction_acceptance`, full `scoped_remote_execution`
and affected Node Registration browser tests.

## Task 9: Terraform objects for the proven billing model

**Files:** Create `enterprise/infra/stripe/{versions,variables,main,outputs}.tf`,
`backend.tf.example` and README; update sandbox billing settings examples.

**Interface:** Output the selected model's Product, Price-per-currency and, for
metered overage, Meter IDs. Use the official `stripe/stripe` provider and a GCS
backend with its own prefix; verify the required provider/API version against
primary documentation when implementing.

- [ ] Model only the selected winner's sandbox Free/Team examples, JPY/USD Prices
      and webhook endpoint. Free remains a local Tier with no required Stripe
      Customer or Subscription. Do not provision both competing billing designs.
- [ ] Run Terraform format, backend-free init and validate; apply only to the
      prepared sandbox. Keep the provider API key in environment configuration and
      signing secrets out of ordinary outputs/logs; handle sensitive state through
      the existing protected backend and runtime Secret Store pattern.
- [ ] Record sanitized object mapping and commit
      `feat(billing): provision the selected Stripe sandbox catalog`.

Run Terraform `fmt -check`, `init -backend=false` and `validate` from
`enterprise/infra/stripe`, followed by the real sandbox plan/apply.

## Task 10: Local payment mirror, durable commands and reconciliation

**Files:** Extend Enterprise `billing/{payments,settings,service,store,models,gate}.rs`,
`billing.rs`, extension composition and classification. Create
`billing/{processor,stripe,webhooks,dispatch}.rs`, migration and
`enterprise/server/tests/stripe.rs`.

**Interfaces:** Keep `PaymentState::subscription` local. Add async
`PaymentProcessor::{checkout,change_tier,deliver_usage,subscription}` with explicit
Checkout/TierChange/UsageBatch inputs and typed receipts. Replace the synchronous
Tier command with `request_tier_change(owner, account, tier) -> PendingChange`:
persist intent/outbox in the capacity transaction, return HTTP 202, send only
after locks are released, and confirm from processor reports.

- [ ] Extend PostgreSQL tests with event-ID deduplication, reordered reports,
      account/customer mismatch, retry after send-before-ack, fixed 14-day grace,
      suspension/recovery and a processor that blocks or fails.
- [ ] Implement only the selected production adapter. For an upgrade, retain the
      current cap until confirmed. For a pending downgrade, use the smaller current/
      target cap, then release the pending cap on definitive failure. Concurrent Node
      creation must remain independent of network delay and preserve capacity.
- [ ] Add signature-verified webhook ingestion, durable outbox/reconciliation and
      local subscription projection. Mount endpoints/run workers only on Cloud;
      Dedicated retains metering/CSV without processor composition.
- [ ] Run `cargo test --locked --manifest-path enterprise/Cargo.toml -p aidash-enterprise-server --test billing --test stripe`; commit
      `feat(billing): mirror Stripe state through durable Cloud commands`.

## Task 11: Correct usage attribution and lifecycle metering

**Files:** Extend Enterprise `billing/{meter,store,models}.rs` and migrations,
application `usage.rs` only if source identity needs generalization, server
`execution/services/storage_usage.rs`, application operation withdrawal and
reconciliation, and the runner's allocation/termination journal.

**Interfaces:** Preserve `UsageEvents::{pod_time,storage_sampled}` and operation
deduplication. Persist account coverage intervals and a per-account/currency/
period export ledger with durable idempotency keys. Current attachments alone
cannot assign earlier usage after a Tenant moves Billing Accounts.

The existing operation withdrawal path must durably deliver confirmed usage even
after authority is revoked. Command start/finish timestamps are not proof of Pod
allocation lifetime, particularly when Python operations reuse a Pod. Use actual
allocation start/stop evidence and one stable UUID per allocation. A recreated
Pod gets another UUID; retries retain the UUID. Do not send a partial interval
under the same ID and expect the current deduplication to extend it later.

For Issue 405, use this same port for the complete allocation interval through
actual stop, including time after the owning Run completes. The preview provider
must retain confirmed timestamps and undelivered reports across restart.

- [ ] Test the generic allocation consumer through a simulated Run-completion
      instant, repeated stop reports,
      month-boundary/account-move splits, export retries, daily sampling by two
      replicas and a purge that changes storage from nonzero to zero.
- [ ] Test real operation withdrawal/revocation and stop-report retries at the
      owning journal boundary. Once Issue 405 is available, verify an actual preview
      remains responsive after Run completion and its complete start–stop interval
      agrees with local and selected-model usage. Keep this external acceptance
      dependency explicit until the runtime exists and the scenario runs.
- [ ] Record real Pod start–stop once, split billable time by coverage and period,
      preserve the existing usage/CSV contract and export through the selected model.
      Emit explicit daily zero samples for active Tenants so stale nonzero usage
      cannot survive physical purge.
- [ ] Verify deleted Node files remain counted until purge, sibling files remain
      counted, and deleting/restoring the whole Tenant respects the existing billing
      exemption. Commit `fix(billing): attribute retained storage and Pod usage exactly once`.

Run Enterprise billing/Stripe tests and the owning storage, allocation journal
and withdrawal tests. Actual preview acceptance also requires Issue 405's runtime.
Do not rerun cryptographic negative cases here.

Task 11a local evidence: the physical-purge regression failed with stale 4,000
bytes before the repair, then passed; all 12 existing billing tests passed.
Independent review approved the scoped sampler change, integrated as
`639c182bc8`. Actual Node purge, allocation lifetime, coverage attribution and
processor export remain separate acceptance requirements above.

## Task 12: Billing dashboard and actual sandbox end-to-end acceptance

**Files:** Extend Enterprise `billing/{views,service}.rs`, route catalog,
`enterprise/web/src/slots/{billing,account-menu}.tsx`, add billing panel and
`enterprise/web/tests/billing.spec.ts`. Generate the API client; add an explicit
`stripe_sandbox` test target and operations instructions.

**Interfaces:** Add owner Checkout and pending-change endpoints, a
`GET /api/tenants/{tenant}/billing` notice visible under Tenant authority, and the
signed Stripe webhook route. A Tenant Administrator need not own the paying
Billing Account to see that Tenant's grace/suspension notice.

- [ ] Verify fake-backed browser flows for accounts, JPY/USD choice, Checkout
      return/pending state, both attachment-consent orders, rejected Tier changes,
      dunning, suspension and resumption. Show inference BYOK, measured vs estimated
      vs unknown costs, preview lifetime and retained Node storage charges distinctly.
- [ ] Run real sandbox Checkout and local confirmation, attachment/move, Tier
      changes, selected-model usage/amount reconciliation, payment failure, 14-day
      local grace expiry, refusal of new work, reading/export and recovery. Verify
      Stripe unavailability does not block local admission. Never use sandbox email
      receipt arrival as an acceptance signal.
- [ ] Export sanitized request/event/object IDs, local/Stripe state comparisons,
      quantities/currency, browser evidence and retry uniqueness; commit
      `feat(web): complete Cloud billing and sandbox acceptance`.

Run `npm run generate:api --prefix enterprise/web`, Enterprise `typecheck`, `lint`
and `build`, plus the new `test:billing` script and explicit ignored sandbox test.
Ordinary CI must not require sandbox credentials.

## Task 13: Full remaining-Issue acceptance and one-PR delivery

- [ ] Reconcile the latest develop/PR 478 state and the complete Issue acceptance
      inventory, including closed descendants. Fix failures at their owning boundary;
      do not infer completeness from a merged PR or stale checklist.
- [ ] Run full `scoped_remote_execution`, transaction suites, relevant identity,
      execution, persistence and Federation partitions; both workspaces' clippy with
      `--workspace --all-targets -- -D warnings`; migration drift and classification.
- [ ] Run generated API/Bruno checks, Community and Enterprise web checks,
      workspace boundaries/alignment, Community export/bundle isolation and the
      relevant browser/Federation acceptance. Check existing Cloud/Dedicated Shared
      Pool sign-in evidence and perform missing live acceptance when available.
- [ ] Update `docs/protocol.md`, `docs/authorization.md`, ADR 0028's stale
      same-Tenant registration wording, ADR 0064's single-key wording and relevant
      operations/settings examples. Mark only actually verified acceptance complete.
- [ ] Obtain a fresh whole-branch review, repair actionable findings, commit and
      push normally. Create/update the one draft PR with the final scope, relevant
      closing references, live `enhancement`/`breaking-change` labels and Codex
      attribution. Verify labels and local/upstream/remote/PR-head SHA agreement.
- [ ] Inspect exact-head hosted CI and report local proof, hosted proof, review
      closure, mergeability and sandbox/live acceptance separately. Missing keys,
      unresolved PR 478 integration or failed/pending acceptance keeps work incomplete.

Canonical broad Rust runners use `scripts/test-rust.sh --partition identity`,
then the same command for `execution`, `persistence` and `federation`. Integration
runners are `scripts/test-acceptance.sh` and
`scripts/test-acceptance.sh --browser-suite`.
The Rust runner establishes the existing capability profile, service images,
stack size and test concurrency. On macOS, preserve those settings when running
the named full targets directly; do not mistake a filtered run for a full target.
