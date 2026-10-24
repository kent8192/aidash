# Complete Issue 244 and all of its remaining sub-issues

Status: product decisions accepted in two design rounds on 2026-10-23. The
written design passed the independent builder check, and the implementation plan
is ready for execution. Stripe model selection awaits the accepted real-sandbox
experiment, not a further speculative design choice.

## Intent and delivery

Complete the remaining work of [Issue 244] and every current descendant in one
new pull request against `develop/0.1.0`. The earlier delivery excluded Node
deletion and Deployment Key rollover; the user's request on 2026-10-23 includes
all current sub-issues, so [Issue 388] and [Issue 390] are included here.

Use branch `feat/issue-244-complete-sub-issues`, starting at
`37d427e09b519fbede493d01997e2447472126dd`. The earlier task branches belong to
already merged pull requests; their work is inherited through develop.

The product separates Deployment administration, Tenant administration and
ordinary membership, with Tenant isolation, Node-owned execution, and Cloud
billing. Existing accepted decisions remain authoritative unless this document
explicitly changes one through an accepted design round.

## Verified baseline

The GitHub hierarchy was checked recursively on 2026-10-23: 15 descendants,
including five open Issues. The local source audit used the base revision above;
the audit did not execute the application test suites.

| Issue | Observed delivery                                                                      | Remaining work                                                                                        |
| ----- | -------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| 239   | Delete/restore/leave in PR 351; purge in PR 423; purge-order repair in PR 428          | Account for the existing restoration repair in PR 478, verify full acceptance and reconcile the Issue |
| 241   | Nodes, Deployment Keys, Node Registrations, protocol 0.4 and dashboard in PR 444       | Verify the remaining execution paths and all acceptance; fix failures                                 |
| 242   | Enterprise billing core, usage events, limits, account consent and a fake payment port | Stripe spike and chosen adapter, Terraform, billing UI and Stripe sandbox acceptance                  |
| 388   | No Node deletion lifecycle                                                             | Design, implement and verify Node deletion, restore and purge                                         |
| 390   | Registrations pin one key                                                              | Design, implement and verify trusted key rollover                                                     |

Issues 243, 238, 240, 223 and the descendants 355 and 262–266 are closed. Their
delivered behavior is part of regression acceptance; closure is not proof that
every hosted or live acceptance test was run.

PRs 349, 351, 384, 389, 423 and 444 are merged. PR 444's description records a
previous `scoped_remote_execution` run of 16/93 before later fixes and no final
full rerun in that implementation session. This is an evidence gap to verify,
not evidence that the merged revision still fails 77 tests.

PR 478 already contains the Plan Connection retirement/restoration serialization
repair tracked by Issue 391. Track its current integration and ownership rather
than independently rewriting that fix.

## Approaches and accepted decisions

The user accepted every recommendation shown in rounds 1 and 2 with “全て推奨”.
This acceptance applies to the choices below; it does not supply missing
credentials or establish a successful sandbox result.

### Recoverable Node deletion

Use a configurable deletion grace period with restore, following the Tenant
deletion model. Deleting a Node cancels its running and queued work and refuses
new work and access. Restoring the Node does not automatically restart canceled
Runs. The alternative, immediate final deletion, was rejected because accidental
Node deletion would have no recovery path.

The default Node must be changed to another live Node before deletion. The last
Node of a Tenant cannot be deleted. Allowing a Tenant with no default Node would
change an existing invariant across execution, catalog and dashboard paths.

The accepted lifecycle details below specify grace settings, capacity accounting,
purge authority, authorization restoration and cross-Node references.

### Trusted Deployment Key rollover

For a normal rollover, receivers automatically accept a new key only after
validating an update signed by a currently trusted old key. Manual re-registration
for every planned rotation would defeat Issue 390's continuity goal. Losing the
old key or suspecting its compromise requires receiving administrators to confirm
the replacement; an unverified public key discovered at an endpoint is not a
trust transition.

The accepted rollover details below specify overlap duration, sender activation,
unavailable receivers and expiry. Manual revocation and replacement use the
existing administrator of each record. Cover Node Registrations and Peer Graph
Grants as well as both file-backed and Enterprise KMS-backed keys.

### Live preview Pod usage

Meter and bill Live preview Execution Pods from start to stop, including time
after the owning Run completes. Use the same Execution Pod time meter rather
than creating a new category. Gateway traffic remains subject to the existing
decision to defer its billing choice to Issue 310.

### Stripe model selection

Preserve Issue 242's sandbox-first decision. Try Managed Payments Checkout with a
metered price backed by a Billing Meter. If accepted, implement included usage
plus metered overage. If refused for this integration, implement Usage Credits
bought through one-off Checkout and held by Aidash. Build only the proven winner
behind one port; CI uses a fake.

[Managed Payments documentation] confirms subscription support and restrictions
on invoice items and one-off invoices, but does not establish this account's
metered-price compatibility. A documentation read is not the required sandbox
result.

The expected root `secrets/stripe-test.env` is absent as of this draft. The user
accepted the sandbox-first approach; actual sandbox access remains unverified.
Never record secret values in this document, source control, logs or comments.

## Accepted lifecycle and rollover details

1. **Node grace period.** Introduce a Node-specific setting, default 30 days and
   allowed range 1–90 days. Fix the deadline when deletion is recorded, so later
   setting changes do not unexpectedly shorten existing recovery windows.
2. **Capacity and downgrade.** Count live Nodes toward the Node limits; deleting
   one immediately frees its slot. Restore checks current capacity and refuses
   with a conflict when the Node would exceed the current limit. Never evict
   another Node. Counting deleted Nodes until purge preserves guaranteed capacity
   for restore but can delay a downgrade for the whole recovery period.
3. **Immediate purge authority.** As with the existing Tenant lifecycle, reserve
   bypassing the grace period for Deployment Administrators. Tenant Administrators
   may soft-delete and restore their Nodes. Immediate capacity release removes the
   need to destroy recoverable data solely to downgrade.
4. **Restored authorization.** Reopen preserved Node data and configuration under
   current Tenant policy and current credential/registration state. Do not roll
   back revocations or policy edits made during the deletion period. Registrations
   owned by another Tenant or Deployment remain controlled by their receiver.
5. **Storage during Node deletion.** Keep retained Node-owned files in the active
   Tenant's storage meter until purge physically deletes them. Stop its Execution
   Pod time when its Pods stop. Show the stored bytes, continued storage charges
   and purge deadline before deletion and in the deleted Node view. The existing
   Tenant-deletion storage exemption still applies when the Tenant itself is
   deleted; a Node deletion does not remove that exemption.
6. **Normal rollover schedule and missed overlap.** Publish the successor and
   signed transition, switch signing after 24 hours, and retire verification of
   the old key seven days after the switch. Allow one normal rollover at a time.
   A receiver that did not accept the successor during the overlap needs manual
   confirmation afterwards; do not trust retired keys indefinitely for updates.
7. **Rollover availability.** Carry a bounded transition proof signed by the
   currently trusted key with requests signed by its successor during the overlap.
   Verify it locally. Public discovery remains useful for publication and advance
   refresh, but receiving Federation must not require an HTTP fetch to succeed.

8. **Durable cancellation across restore.** Snapshot the exact work identities in
   the deletion transaction. Runs have no immutable creation timestamp, so later
   maintenance must not guess which work predates deletion from `updated_at`.
   Keep the stop intent through restore, fence old leases and pending work, and
   finish terminal/quota/network cleanup through the existing owners.
9. **Source reads survive source purge as provenance.** Keep a sibling Node's
   content-free source Unit ID and revision. Check source existence under a key
   share lock when recording the read, rather than making its lifetime foreign
   key prevent source purge forever. Invalidate derived receipt bodies through
   the existing source-deletion owner before removing the source. Keep unrelated
   sibling data and read history. Unknown or contradictory ownership still blocks
   erasure. This also avoids mutually retaining Nodes preventing both purges.
10. **Closed-Node recovery learns keys without reopening authority.** A request
    bound to an already-admitted durable transaction may persist a verified key
    transition after the exact participant, manifest and caller are revalidated
    in the same transaction as the trust compare-and-set. Preserve authorization
    revision, permissions and enabled state. A concurrent disable forces reload
    and revalidation; quarantine cannot be cleared by a normal proof. Merely
    accepting the proof temporarily would strand a later retry after overlap.
11. **Physical purge retains exact recovery trust.** Current registration
    pairs referenced by immutable retained journals survive as recovery state:
    local coordinator to remote participant, and local participant to remote
    coordinator. Validate transaction ID, digest, manifest and permanent owner;
    keep current keys, both revisions, quarantine, permissions, enabled state and
    endpoint unchanged. Registration ownership is anchored to permanent Node
    names, allowing finite catalog/deletion/history audit expiry. COMMITTED or
    ABORTED does not establish remote receipt; Finish still needs the reverse
    decision route. Trust remains while its journal is replayable, independently
    of the audit deadline. Unrelated registrations are purged; no participant
    mesh, historical trust fallback, new acknowledgement or fresh admission is
    implied. Current manifests allow only one distinct Node per Deployment, so
    a valid same-Deployment coordinator/participant is self and requires no
    registration. Malformed or contradictory recovery ownership fails closed.
    Non-atomic execution Cancel retries retain only the actual typed terminal
    Activation, exact original binding and required receiver-to-Home trust.
    Capture shares the real response transaction; Rows captures stopped
    admissions before dependency selection, with planned evidence/trust checked
    before files. Ordinary admission/Run payload still erases. Only the exact
    strict empty-Subject Cancel request may replay or learn receiving keys;
    current Tenant policy, lifecycle and quarantine remain authoritative. No
    remote ACK bounds retention, so Node audit expiry does not erase this small
    recovery obligation. Whole-Tenant purge removes it normally.

The alternatives were immediate final deletion, counting deleted Nodes until
purge, letting Tenant Administrators bypass the recovery period, restoring an
old policy snapshot, exempting retained Node storage, keeping retired key trust
indefinitely, and fetching public identity synchronously during admission. The
accepted choices preserve recovery while enabling downgrade, preserve current
authorization and account for retained storage, and keep normal key updates
available without extending retired trust indefinitely.

## Decisions left to the implementer

Choose concrete Rust types, bounded proof encoding, route and settings names,
database selectors and worker batching in the implementation plan, following
existing interfaces and the accepted behavior above. They may not change the
trust model, lifecycle, billing boundary or administrator authority. Recreate
development schemas and fixtures instead of adding legacy compatibility.

For emergency replacement, keep local quarantine and manual re-pin within the
responsible administrator's existing authority. Require an independent confirmation of the new
key. A sender publishing an incident notice does not force every independent
receiver to learn or act on that notice instantly.

## Constraints already settled by the project

- Issue 244 explicitly permits breaking changes without compatibility shims:
  Aidash is not deployed and development databases are recreated. New agent and
  service subject ownership is required in the recreated schema and fixtures;
  do not invent a Node assignment for legacy unqualified service subjects.
- Same-Tenant Federation uses Tenant policy without a Node Registration or Peer
  Mapping. Other-Tenant Federation uses receiving Node registration and mapping.
- Protocol 0.4, request-bound short-lived Federation tokens and the existing
  Deployment-wide Federation setting remain the starting point.
- Tenant data remains protected by Tenant-granular RLS. Node purge must select
  only its Node's data and fail verification if Node-owned tables lack coverage.
- Node names are never reused. Tenant-owned Registry definitions and credentials
  must not be mistaken for exclusively Node-owned data.
- ADR 0028 already places agent and service subjects in one Node. The present
  `Subject` model lacks explicit ownership, so add and validate that ownership
  rather than guessing it from manually assigned service names during purge.
  User subjects remain Tenant-wide.
- Keep Node-owned purge selectors and shared/retained entries explicit. Preserve
  sibling Node data, shared Registry definitions, Provider Credentials, Tenant
  Secrets and shared Tenant policy revisions. Retain separable Node audit under
  the existing finite audit retention setting (365 days by default).
- A normal key rollover changes authentication material, not registration/grant
  permissions or enabled state. Keep authorization revisions stable. Existing
  admitted work may complete and retries authenticate with the current key;
  never unilaterally abort a transaction whose commit is already decided.
- Manual revocation and replacement respect each record's existing management
  boundary. Do not promise instantaneous revocation at independent receivers that
  have not learned of a compromised key.
- Inference remains BYOK. Billing and processor integration live in Enterprise.
  Run admission reads local mirrored state and never waits on Stripe.
- Cloud and Dedicated share the Enterprise build. Dedicated meters and reports
  usage, mounts no billing endpoints and runs no Stripe loops.
- Real launch prices remain outside the sandbox example Tiers. Aidash sends no
  billing email; Stripe sends its notices and the dashboard shows billing state.
- Reconcile old documentation with later accepted decisions, including ADR 0028's
  same-Tenant registration wording and Issue 301's audit-access decisions.

## Completion evidence required

| Area                 | Evidence                                                                                                                                                                         |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tenant lifecycle     | Delete, restore, leave, resumable purge, preservation of another Tenant, restore/purge races and integration of the existing retirement serialization repair                     |
| Nodes and Federation | Full `scoped_remote_execution`, relevant Federation/transaction suites, same-Tenant and cross-Tenant exchange, Node selection and catalog isolation                              |
| Node deletion        | Work cancellation, all admission boundaries, restore without restart, default/last Node constraints, purge coverage, preserved sibling Node and Tenant data, capacity accounting |
| Key rollover         | Normal overlap without interrupted exchange, unknown/unsigned replacement refusal, expiry/revocation, offline receiver handling, file and KMS adapters, Peer Graph Grants        |
| Cloud billing        | Actual sandbox spike, only the selected model, subscribe/Tier changes, both attachment consents, meters, dunning, suspension/resumption and user-facing cost explanations        |
| Offering boundaries  | Community excludes billing code; Dedicated has usage/CSV with no processor routes or loops; Cloud/Dedicated sign-in acceptance evidence                                          |
| Delivery             | Relevant local gates, exact PR head hosted checks, labels, source/remote SHA agreement, completed Issue checklist mapping and documented live acceptance                         |

Running or canceled checks are not passing checks. Missing sandbox credentials
or unperformed live acceptance remain explicit incomplete work; a fake processor
test does not replace Stripe sandbox acceptance.

[Issue 244]: https://github.com/kent8192/aidash/issues/244
[Issue 388]: https://github.com/kent8192/aidash/issues/388
[Issue 390]: https://github.com/kent8192/aidash/issues/390
[Managed Payments documentation]: https://docs.stripe.com/payments/managed-payments
