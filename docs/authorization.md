# Authorization policy service and scoped execution

The policy service implements policy management, decisions, revocable subject credentials, scoped workspace and interaction APIs, tenant catalog approvals, local execution and a scoped remote execution path for FR-AUTH-001. Management endpoints accept the Deployment Administrator bearer token, an explicitly granted dashboard browser Deployment Administrator session, or a Tenant's own subject acting under that Tenant's policy actions (#363). This implementation has focused two-node tests, but the complete remote federation, resource-scope and integrated release acceptance in #38 remains open; code existence is not completion of FR-AUTH-001.

## Dashboard identity

`GET /auth/config` reports whether OIDC or GCIP is configured. `GET /auth/login` starts Authorization Code with PKCE, and `/auth/callback` consumes the browser-bound transaction and creates an opaque Aidash session. Where the Tenancy Rules turn Personal Tenants on, the first sign-in of an External Identity without a Tenant Binding provisions its Personal Tenant: a Tenant under an assigned name, labeled `Personal`, whose initial policy places the identity's `owner` subject in `administrators`, with normal Tenant-scoped authority and no Deployment Administrator grant. A Personal Tenant link records the identity once, so the allocation is stable, and provisioning never restores a disabled Mapping or withdrawn policy. Existing authenticated sessions receive the same allocation on their next `GET /auth/session`. GCIP sign-ins keep their existing enrollment rules.

`GET /auth/session` lists only the signed-in identity's enabled mappings and Deployment Administrator availability; a personal Mapping also has `personal: true`, and each Mapping reports `credential_active: false` when its nonexportable credential is revoked or expired. `personal_account: true` marks an identity that has a Personal Tenant. Each Mapping also reports its Tenant `label`, `tenant_creation: true` tells the dashboard it may offer Tenant creation, and `invitations` reports whether the Deployment issues Invitations. A saved Mapping selection whose credential is inactive is discarded. When the tab has no valid saved authority, the dashboard selects that personal Mapping automatically only while its credential is active; otherwise the authority chooser remains. A saved shared Mapping or explicitly granted Deployment Administrator selection remains intact. Personal use requires no registration request. Authenticated identities may read and submit their own shared-access request through `GET` and `POST /auth/registration`, including while using a personal Tenant; pending approval does not revoke or replace an enabled Mapping. An unmapped legacy identity still needs approval before accessing `/api` data. Browser mutations send the session's `aidash-csrf` value in `x-aidash-csrf`; the backend also checks `Origin`. `POST /auth/logout` revokes the current browser session, while `/auth/logout-all` revokes all Aidash sessions for that external identity. A validated `POST /auth/backchannel-logout` revokes the matching provider sessions. API Bearer authentication remains available and takes precedence when an Authorization header is present.

The caller selects `x-aidash-context: mapping:<tenant>:<mapping-id>` or `deployment_administrator` for each browser API request. The selector names the Mapping's Tenant, percent-encoded so the header stays visible ASCII (every UTF-8 byte outside visible ASCII, and `%`, becomes `%XX`). Subject Credentials name their Tenant the same way. The server rechecks the identity, exact mapping or Deployment Administrator grant, and tenant policy. The UI stores only the non-secret selector per tab; the session credential stays in an HttpOnly cookie. Direct Google users can request shared Tenant access from **Account settings**, including after their personal Mapping is disabled. Deployment Administrators can list `/api/dashboard/registrations?offset=0` (200 pending requests per page, newest first), `/api/dashboard/identities`, `/api/dashboard/mappings` and `/api/dashboard/deployment-administrator-grants`; approve a pending request with `POST /api/dashboard/registrations/{id}/approve` and `{"tenant":"acme","subject":"alice"}`, reject one at `/reject`, disable a mapping at `/api/dashboard/mappings/{id}/disable`, and set an identity's Deployment Administrator grant at `/api/dashboard/identities/{id}/deployment-administrator-grant` with `{"enabled":true|false,"expected_revision":0}` for creation or the current revision for an update. Shared approval requires an existing enabled user subject and never grants Deployment Administrator access. The grant list and update response include the revision; a stale update returns 409. A pending registration expires after seven days and can be resubmitted. An approved request remains in the audit history; a direct Google identity can submit another request to join an additional shared Tenant, while Keycloak and GCIP retain their existing enrollment restrictions. If the identity's last mapping is disabled, it can request approval again; approval can restore the same subject with a new nonexportable credential. The first Deployment Administrator grant uses the existing Bearer recovery credential. A Keycloak-disabled identity requires a successful service-account status check at `/api/dashboard/identities/{id}/restore` before a fresh sign-in is permitted.

OIDC-origin execution stores an immutable run origin and rechecks its underlying nonexportable credential, mapping, external identity validity and subject policy at execution boundaries. Logging out of Aidash does not cancel admitted work. For legacy Keycloak deployments, an outage beyond the 15-minute validity deadline pauses work; a fresh successful status check can resume only work waiting for that outage and only while all local authority remains valid. A definite Keycloak disablement revokes browser sessions and requires a Deployment Administrator identity restore followed by an authorized manual run resumption. Google identities are authenticated at sign-in and governed by local authority thereafter; Google has no equivalent service-account account-status lookup, so Google account changes do not automatically pause work or revoke existing sessions. Session expiry and local mapping, deployment-administrator-grant and credential revocation still apply. Identity restore for Google is a local Deployment Administrator action and does not attest to current Google account status; a new session still requires Google authentication. Existing Bearer-origin work retains its credential semantics.

### GCIP sign-in

GCIP is opt-in and replaces direct Google OIDC for a Cloud deployment. A deployment selects exactly one issuer. An External Identity is keyed by issuer, GCIP Tenant ID and uid; OIDC rows use an empty GCIP Tenant ID and retain their issuer/subject identity. Emails, names and GCIP custom claims never link identities or grant authority.

A GCIP Tenant's MFA Requirement (`disabled`, `enabled` or `mandatory`) is enforced by GCIP and completed in the sign-in page with a TOTP code. The token verifier ignores `firebase.sign_in_second_factor` and `second_factor_identifier`: a token that carries them and one that does not produce the same verified sign-in, so a second factor is never authority and never changes a Mapping, Deployment Administrator grant or Account Status.

Display Attributes (verified email and display name) are stored only on the External Identity, visible only to Deployment Administrators through `/api/dashboard/identities` and to Tenant Administrators of the Tenant bound to the identity's GCIP Tenant, never logged, and overwritten at each sign-in. They are kept only while the External Identity is not disabled and has a retention basis:

- an enabled Mapping;
- an enabled Deployment Administrator grant;
- an unexpired pending Registration Request;
- a live browser or desktop session (unrevoked and within its absolute and idle limits) that started after the latest Registration Request ended (its decision, or its deadline when it expired);
- a sign-in within the last ten minutes, after that request ended.

Rejection clears them immediately when no basis remains. Otherwise, expiry, an abandoned sign-in without a request, Mapping or deployment-administrator-grant disablement, and Identity Disablement are cleared by the 60-second sweep or the applicant's next `GET /auth/registration`. The sweep handles each External Identity in its own transaction under its identity lock and rechecks every basis there, so it never erases claims from a sign-in that is still starting its session. The sweep keeps running when dashboard sign-in is not configured, and a bearer Deployment Administrator can still reject pending requests then. No session is usable or still starting in that state, so neither sessions nor recent sign-ins retain anything. A later sign-in records the attributes again, except a rejected sign-in of a disabled External Identity, which records none.

`POST /api/dashboard/identities/{id}/display-erasure` performs a Deployment Administrator's Display Erasure: it clears both attributes and records `display_erased_at` and `display_erased_by` (`oidc:<identity-id>` or `deployment-administrator-bearer`), never the erased values. Display Erasure is permanent, idempotent (a repeat keeps the first trace), and later sign-ins no longer record Display Attributes for that External Identity. It retains the External Identity, its Mappings, Registration Requests and authorization decisions, none of which copy Display Attributes, so authorization history stays verifiable. Database backups keep earlier values until they expire under NFR-DATA-001.

A Tenant Binding maps a GCIP Tenant ID to exactly one Aidash Tenant name. Registration approval and each request, durable execution lease and idle stream snapshot require a Mapping's Tenant to equal the bound Tenant. Removing a Binding causes Identity Disablement at the next boundary, even inside the five-minute freshness window. Installing GCIP policy reconciles every enabled Identity before request/worker admission, including those without a live session or nonterminal run. The periodic refresh repeats that reconciliation. A verified login also disables an inactive existing Identity when it observes the removed Binding, before contacting that pool's Admin API. Restoring the Binding alone does not restore authority. Restore requires a Deployment Administrator and a valid current Binding; admitted work then needs an authorized manual resumption.

`GET /auth/login?org=<Tenant>` records the state, browser-cookie binding, chosen GCIP Tenant, transaction start, return path and desktop consent handoff for ten minutes. `/sign-in` loads the npm SDK only for GCIP. `GET /auth/gcip/transaction` exposes the selected pool's public SDK configuration only to that browser. `POST /auth/gcip/exchange` requires the browser transaction and exact `Origin`, RS256 JWKS signature, exact securetoken project issuer/audience, expiry, selected bound pool, and `auth_time` no more than 30 seconds before transaction start. The backend requires the signed `firebase.sign_in_provider` to appear in that pool's configured provider list before Admin lookup or session admission, including for an existing uid. Password tokens require `email_verified`. Before session admission, the exchange performs a live tenant-scoped Admin lookup, consumes the transaction once and starts an opaque Aidash session; all auth responses carry `Cache-Control: no-store`.

A Sign-in Domain maps a verified email domain to one bound GCIP Tenant. It only routes sign-in and never grants authority, matches or links identities. `POST /auth/login` accepts a form-encoded `email` and optional `return_to`, requires the exact `Origin`, and derives the domain. The address is never stored or logged. Matching is exact after lowercasing and IDNA conversion; subdomains do not inherit their parent's route. A mapped domain starts the same transaction as `?org=`. An unmapped domain, a malformed address and an unknown Tenant name all receive the same `303` to `/sign-in?error=not_found&return_to=…`, with no transaction and no cookie. Neither response names the Tenant. Once routed, the browser necessarily learns the GCIP Tenant ID and provider IDs it needs for the SDK. Removing a Sign-in Domain only stops routing: identities, sessions, work and in-flight transactions are unaffected. Staff verify domain ownership outside Aidash before adding it to Terraform, and review rejects public mail domains. `POST /auth/login` and `GET /auth/login?org=` share a per-IP discovery budget that is separate from the rest of `/auth/*`; see [HTTP limits](operations/http-observability.md#configuration).

Password account creation sends a verification email before sign-in. If delivery fails after creation, the page reports that the account exists and offers **Resend verification email**. Resending authenticates the existing account with its email/password and works after reloading the page; it never sends an unverified token to Aidash. Each attempt clears the password and closes the SDK client, with ID and refresh tokens held only in memory.

Federated sign-in opens a popup. Only when the browser blocks it (`auth/popup-blocked`) does the page switch to a redirect; closing the popup is a failure, not a fallback. The redirect returns to `/sign-in?state=…`, so the same browser-bound transaction, cookie binding and `auth_time` window apply. The page completes a pending redirect before showing the form. With `auth_helper = "public_origin"`, Firebase's sign-in helper runs on the Aidash origin, so it can write `firebase:` keys there. After each attempt, successful or not, the page removes every `firebase:` key from local and session storage and deletes Firebase's IndexedDB store. The helper path grants no authority; only the exchange admits a session.

Account Status carries both `disabled` and `validSince`. A disabled/deleted account blocks sign-in, disables the Identity, revokes sessions and pauses admitted work at its next boundary. Re-enabling GCIP alone cannot restore Aidash authority. An advanced `validSince` atomically revokes only sessions whose original `auth_time` is strictly earlier; it leaves admitted work and Mapping credentials active. Desktop sessions inherit the browser's authentication time. A successful status result is fresh for five minutes; during an Admin outage the last successful result can admit boundaries for up to fifteen minutes. Beyond that deadline sessions are unavailable and work waits for status recovery, retaining the existing automatic outage-only recovery rules.

The tenant-scoped effect of `firebaseauth.users.get`, applicability of the configured viewer role on tenants, and which password/account events advance `validSince` still require a GCP sandbox. The implementation consumes the returned timestamp without inferring those events. No unsigned/emulator token path exists.

### Tenant Administrators

A Tenant Administrator approves Registration Requests, manages Mappings, and reads and replaces the policy of its own Tenant, without a Deployment Administrator. It is not a separate account or grant record. It is an External Identity acting through one of its Mappings, whose subject the Tenant's policy permits to perform these actions:

| Action                          | Resource kind  | Allows                                                                                                         |
| ------------------------------- | -------------- | -------------------------------------------------------------------------------------------------------------- |
| `registration.read`             | `registration` | List pending Registration Requests addressed to the Tenant, with Display Attributes                            |
| `registration.approve`          | `registration` | Approve a request into a new user subject                                                                      |
| `registration.reject`           | `registration` | Reject a request                                                                                               |
| `mapping.read`                  | `mapping`      | List the Tenant's Mappings with Assignable Group memberships (and, under a Tenant Binding, Display Attributes) |
| `mapping.disable`               | `mapping`      | Disable another External Identity's Mapping                                                                    |
| `subject.group.update`          | `subject`      | Replace a user subject's Assignable Group memberships                                                          |
| `invitation.create`             | `invitation`   | Issue an Invitation link for the Tenant                                                                        |
| `invitation.read`               | `invitation`   | List the Tenant's Invitations and their state                                                                  |
| `invitation.revoke`             | `invitation`   | Revoke an Invitation that has not been used                                                                    |
| `policy.read`                   | `policy`       | Read the policy and its revisions, and simulate a decision                                                     |
| `policy.update`                 | `policy`       | Replace the whole policy                                                                                       |
| `decision.read`                 | `decision`     | Read the decision log                                                                                          |
| `credential.issue`              | `subject`      | Issue a Subject Credential for a service or agent subject no Mapping ever named                                |
| `credential.read`               | `credential`   | List every Subject Credential of the Tenant                                                                    |
| `credential.revoke`             | `credential`   | Revoke any Subject Credential of the Tenant                                                                    |
| `peer_mapping.read`             | `peer_mapping` | List the Tenant's Peer Mappings and their history                                                              |
| `peer_mapping.update`           | `peer_mapping` | Create, update and disable a Peer Mapping                                                                      |
| `workbench.test_limits.update`  | `test_limits`  | Read and set the Tenant's Workbench Test Limits within the Deployment's ceilings                               |
| `workbench.test_profile.update` | `test_profile` | Read, with their rules, and write the Tenant's Workbench Test Profiles                                         |

Item operations evaluate the item's ID (the request, Mapping or subject). Lists and `GET /api/tenants/{tenant}/administration` evaluate the resource ID `all`. Item operations also pass resource attributes for conditions: `registration.approve` passes `identity_id`, `subject` and the requested `groups`; `registration.reject` passes `identity_id`; `mapping.disable` passes `identity_id` and `subject`; `subject.group.update` passes the requested `groups`. A deny for some administrators whose condition is that `/groups` contains an Assignable Group therefore keeps them from conferring that group, at approval as well as on membership updates. Decisions, including denials, are recorded in the Tenant's decision log.

A Tenant Administrator exists in any Tenant whose policy permits these actions; it needs no Tenant Binding. A Registration Request reaches a Tenant Administrator only when it is addressed to that Tenant: through a Tenant Binding (GCIP), whose requests carry the bound Tenant, or through an Invitation. A request addressed to no Tenant is decided only by Deployment Administrators. Display Attributes are shown only for people who chose the Tenant: those whose requests were addressed to it, pending or decided, and its Tenant Creator. Every Tenant Administrator route requires a browser session in a `mapping:<tenant>:<mapping-id>` context whose Tenant equals the path Tenant; the Deployment Administrator bearer token, a Deployment Administrator context and Subject Credentials are refused, except that Deployment Administrators may also issue, list and revoke Invitations for any Tenant.

The Tenant's policy marks the groups a Tenant Administrator may confer without replacing the policy, by setting `"assignable": true` on a group. A subject holding `policy.update`, or a Deployment Administrator, sets the mark. Without `policy.update`, these Assignable Groups are the only authority a Tenant Administrator can grant:

- Approval creates a new `user` subject with only the requested Assignable Groups, no roles and no delegator. It cannot hand out an existing subject (409), because that subject may hold authority outside the Assignable Groups. The one exception is re-approving an External Identity into the subject it was previously mapped to; a re-approval sends no groups and keeps the subject unchanged.
- `PUT /api/tenants/{tenant}/subjects/{subject}/groups` with `{"groups":[...],"expected_policy_revision":n}` replaces only Assignable Group memberships. Other groups and roles are retained.
- Neither operation edits roles, group definitions or any other part of the policy. Those change only when the whole policy is replaced.

A Tenant created through Aidash starts with two Assignable Groups: `administrators`, allowed every action, and `members`, allowed an explicit list of actions to use the Tenant and author definitions. Tenant Administrators can therefore appoint each other; unmarking `administrators` stops that. A Tenant Administrator cannot disable its own Mapping or change its own subject's groups (403). Before a Tenant Administrator disables a Mapping, changes Assignable Groups or replaces the policy, the same transaction checks that another enabled Mapping of an enabled External Identity keeps every administration action without conditions; otherwise it answers 409. A replacement counts the actor's own Mapping. The administration actions are `registration.read`, `registration.approve`, `registration.reject`, `mapping.read`, `mapping.disable`, `subject.group.update`, `invitation.create`, `policy.read` and `policy.update`, so a Tenant always keeps someone who can repair its policy (ADR 0026). Conditional allows, and allows limited to particular resources, do not count, and every deny counts as matching. Deployment Administrator actions, including replacing the whole policy, are never blocked.

Subjects created, memberships changed and policies replaced by a Tenant Administrator advance the policy revision with the actor `oidc:<identity id>` and the Mapping it acted through (`actor_mapping_id`). A browser Deployment Administrator's replacement records `deployment_administrator:oidc:<identity id>`. A Deployment Administrator replacing the policy at an older `expected_revision` receives 409 and must reload.

| Method | Path                                               | Body                                                                                                       |
| ------ | -------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| GET    | `/api/tenants/{tenant}/administration`             | Returns the permitted `actions`; for a Tenant Administrator also `assignable_groups` and `policy_revision` |
| GET    | `/api/tenants/{tenant}/registrations?offset=0`     |                                                                                                            |
| POST   | `/api/tenants/{tenant}/registrations/{id}/approve` | `{"subject":"carol","groups":["members"]}`                                                                 |
| POST   | `/api/tenants/{tenant}/registrations/{id}/reject`  |                                                                                                            |
| GET    | `/api/tenants/{tenant}/mappings?offset=0`          |                                                                                                            |
| POST   | `/api/tenants/{tenant}/mappings/{id}/disable`      | `{"expected_revision":1}`                                                                                  |
| PUT    | `/api/tenants/{tenant}/subjects/{subject}/groups`  | `{"groups":[],"expected_policy_revision":2}`                                                               |

Responses forbid caching. In the dashboard, a subject who selects that Mapping sees the permitted parts of **Access and accounts**: its own API tokens for every member, then the administration and policy panels its Tenant's policy permits. Restore and Deployment Administrator grants are not shown. Each listed Mapping carries `own_identity` (the caller's External Identity holds it, so it cannot be disabled) and `own_subject` (the caller acts as, or was ever mapped to, that subject in this Tenant, so its groups cannot be changed). The dashboard omits the Disable and group controls these flags rule out.

Approvals, rejections, Mapping disables, group changes, policy replacements (`policy.update`), Tenant creation, label changes and Invitation issue, revocation and redemption are appended to `dashboard_administration_history`. Each row records the action, the External Identity it concerns (if any), request, Mapping, Tenant, subject and Invitation, the actor kind (`deployment_administrator`, `tenant_administrator` or `identity`), the acting External Identity (absent for the Deployment Administrator bearer token) and the Mapping it acted through. Rows are never updated.

### Tenancy Rules, Tenant creation and Invitations

Every Tenant has a root record keyed by its Tenant name, `<adjective>-<noun>-<8 random characters>` such as `brave-otter-k3m9q2xa`. Aidash assigns the name, it never changes and it is never reused; the root record outlives purge with only its label cleared. A policy replacement at `expected_revision: 0` for a Tenant without a root record answers 404, so `POST /api/authorization/{tenant}` never creates a Tenant.

The Tenancy Rules are deployment-wide settings under `[tenancy]`; no Tenant's policy decides them. Identities of a GCIP Tenant with a Tenant Binding are never covered: they hold no Personal Tenant, create no Tenants and redeem no other Tenant's Invitation.

| Setting             | Default | Meaning                                                                   |
| ------------------- | ------- | ------------------------------------------------------------------------- |
| `personal_tenants`  | `false` | Provision a Personal Tenant at an identity's first sign-in                |
| `tenant_creation`   | `false` | Let signed-in External Identities create Tenants                          |
| `creation_per_hour` | `5`     | Creations per External Identity within any 60 minutes                     |
| `creation_total`    | `20`    | Tenants one External Identity created and has not had purged              |
| `invitations`       | `true`  | Let Tenant Administrators and Deployment Administrators issue Invitations |

`POST /api/tenants` with `{"label":"Research"}` creates a Tenant and answers 201 with `{name, label, created_at, mapping_id}`. The label must have 1 to 64 characters after trimming and no control characters. The route authenticates itself:

- The Deployment Administrator bearer token, or a browser session in the `deployment_administrator` context, creates a Tenant without limits and maps nobody into it. Its first administrator joins through an Invitation the Deployment Administrator issues and approves (`POST /api/dashboard/registrations/{id}/approve` with `groups` creates the subject in any group), or, for a Tenant with a Tenant Binding, through the bound pool's request. The dashboard offers the Invitation controls in the Deployment Administrator's view of a Tenant, and `GET /api/dashboard/registrations` reports each request's `invitation_id` and `proposed_groups`, from which the approval form starts; a request without an Invitation starts from no groups, so `administrators` is always an explicit choice.
- Any other browser session creates a Tenant for its External Identity, without needing a Mapping, where `tenant_creation` allows it. The identity becomes the Tenant Creator: it receives the first Mapping, to `owner` in `administrators`, and the first policy revision's actor is `oidc:<identity>`. Exceeding `creation_per_hour` answers 429 with `Retry-After` until a slot frees; exceeding `creation_total` answers 403 `tenant_limit_reached`. Personal Tenants never count. An advisory lock per identity serializes concurrent creations.
- Subject Credentials, agents and peers never create Tenants (403).

`GET /api/tenants?offset=0` lists root records for Deployment Administrators, newest first, 200 per page. `PUT /api/tenants/{tenant}/label` changes the label under `tenant.update`, through a browser Mapping of that Tenant.

An Invitation is a single-use, expiring link. `POST /api/tenants/{tenant}/invitations` with `{"groups":["members"],"expires_in_seconds":86400}` issues one under `invitation.create`, or as a Deployment Administrator. The groups must be Assignable Groups and default to `members`; the lifetime defaults to, and may not exceed, 7 days. At most 100 Invitations of a Tenant are active at once. The response carries `link`, `https://<origin>/invitations/<tenant>#<secret>` with the Tenant name percent-encoded as one path segment, and the secret, only this once; Aidash stores a SHA-256 digest. The secret travels in the URL fragment, so it never reaches server logs. `GET /api/tenants/{tenant}/invitations` lists them with their state (`active`, `redeemed`, `revoked` or `expired`), and `POST /api/tenants/{tenant}/invitations/{id}/revoke` revokes an unused one.

Whoever holds a link signs in first. `POST /auth/invitations/inspect` with `{tenant, secret}` shows the inviting Tenant's label and spends nothing. `POST /auth/invitations/redeem` spends the link on a pending Registration Request addressed to that Tenant, which a Tenant Administrator or Deployment Administrator still approves; the request lasts 7 days and carries the Invitation's proposed groups for the approval. The first redemption spends the link whatever happens to the request. The redemption share-locks the browser session after the identity, so a logout committing meanwhile answers 401 and spends nothing. A disabled identity, an identity with a Tenant Binding, an identity already holding an enabled Mapping in the Tenant or already waiting for it, and turned-off Invitations are refused without spending it. Expired, revoked and spent links all answer 404 `invalid link`. An identity has at most one pending request without an Invitation, plus one pending request per Tenant; the 24-hour wait after a rejection applies only to the former.

### Switching issuers

Issuer changes require a maintenance window and a coordinated stop/start. This applies to Keycloak-to-Google migration and rollback. A rolling restart is unsafe: each server and worker refreshes active identities, and a process using a different issuer disables those identities and revokes their sessions.

1. Block dashboard sign-in and stop every Aidash server and worker sharing the database. Confirm that no process remains on the old configuration, including standalone workers and replicas on other nodes.
2. Update the issuer and its required credentials consistently across all replicas. Restart them only after all old processes have stopped, then reopen dashboard sign-in.
3. Have users sign in with the new provider. Where the Tenancy Rules turn Personal Tenants on, sign-in provisions new Personal Tenant authority; shared mappings and Deployment Administrator grants still require explicit approval. Identity records, grants and mappings are not linked by email or transferred between issuers. Old-issuer sessions lose access, and work that depends on the old identity can pause.
4. If mixed configurations already disabled an identity, first complete the coordinated restart. Use the Deployment Administrator bearer recovery credential to restore the affected identity through `POST /api/dashboard/identities/{id}/restore`. Revoked sessions require a fresh sign-in; paused work requires an authorized manual resumption.

## Management API

| Method | Path                                                  | Behavior                                                                  | A Tenant's subject needs                                              |
| ------ | ----------------------------------------------------- | ------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| POST   | `/api/authorization/{tenant}`                         | Validate and atomically replace a policy bundle using `expected_revision` | `policy.update`                                                       |
| GET    | `/api/authorization/{tenant}`                         | Read the current bundle and revision                                      | `policy.read`                                                         |
| POST   | `/api/authorization/{tenant}/evaluate`                | Evaluate against the current revision and persist a decision audit        | Deployment Administrators only                                        |
| POST   | `/api/authorization/{tenant}/simulate`                | Evaluate without writing an audit or executing an operation               | `policy.read`                                                         |
| GET    | `/api/authorization/{tenant}/revisions`               | Read immutable policy history using `after` (revision) and `limit`        | `policy.read`                                                         |
| GET    | `/api/authorization/{tenant}/decisions`               | Read decision history using `after` (sequence) and `limit`                | `decision.read`                                                       |
| POST   | `/api/authorization/{tenant}/credentials`             | Issue a subject bearer token, returned once                               | `credential.create` for its own subject, `credential.issue` otherwise |
| GET    | `/api/authorization/{tenant}/credentials`             | List the latest 200 credential metadata records, without tokens or hashes | `credential.read`; none with `mine=true`                              |
| POST   | `/api/authorization/{tenant}/credentials/{id}/revoke` | Idempotently revoke a credential in the named tenant                      | `credential.revoke`; none for its own                                 |

A Deployment Administrator may call every route for any Tenant. A Tenant's own subject calls them from a browser session in a `mapping:<tenant>:<mapping-id>` context whose Tenant equals the path Tenant, under the action in the last column; a Subject Credential is refused, so a leaked token cannot extend itself. A denial is answered with 403 and recorded in the Tenant's decision log. `evaluate` stays with Deployment Administrators because a persisted evaluation cannot be told apart from a real decision in that log; `simulate` answers the same question without writing it. A Tenant's subject holding `policy.update` holds every authority within the Tenant, including over its own subject; the replacement is checked like any other Tenant Administrator change (ADR 0021 and ADR 0026, amended). The catalog routes below belong to the Tenant: a Deployment Administrator reaches no Tenant's Registry or catalog (ADR 0029).

Creation requires `expected_revision: 0`. Subsequent changes require the current revision. Concurrent updates with the same revision cannot both succeed; the loser receives HTTP 409. Revisions and their full policy documents are committed together. History pages default to 100 records and are capped at 200.

Example creation body:

```json
{
  "expected_revision": 0,
  "bundle": {
    "tenant": "acme",
    "subjects": {
      "alice": {
        "kind": "user",
        "roles": ["researcher"],
        "attributes": { "team": "research" }
      }
    },
    "roles": { "researcher": {} },
    "policies": [
      {
        "id": "team-workspace-read",
        "effect": "allow",
        "subjects": { "roles": ["researcher"] },
        "actions": ["workspace.read"],
        "resources": { "kinds": ["workspace"] },
        "condition": {
          "op": "eq",
          "left": { "source": "subject", "path": "/team" },
          "right": { "source": "resource", "path": "/team" }
        }
      }
    ]
  }
}
```

Example evaluation body:

```json
{
  "subject": "alice",
  "action": "workspace.read",
  "resource": {
    "tenant": "acme",
    "kind": "workspace",
    "id": "workspace-1",
    "attributes": { "team": "research" }
  },
  "environment": { "network": "vpn" }
}
```

An evaluation is a caller-supplied scenario, not a credential or permission to execute the named action. Execution integrations must resolve resource and environment attributes from trusted state and inspect `Decision.allowed` before operating.

## Subject credentials and workspace access

Issue a credential with `{"subject":"alice","expires_in_seconds":3600}`. The subject and every delegation ancestor must exist and be enabled. Lifetime defaults to one hour and must be between one second and 30 days. The response contains `credential` metadata and a single `token`; store the token when receiving it. PostgreSQL stores only its SHA-256 digest. Authenticated responses include `Cache-Control: no-store`.

Who may issue one depends on its subject (ADR 0063):

- A member issues a credential for the subject its own Mapping acts as, under `credential.create`, which the `members` group holds. The credential records that Mapping (`mapping_id`) and ends with it: disabling the Mapping, whether by Leaving a Tenant, a Tenant Administrator or a Deployment Administrator, revokes it in the same transaction, and every boundary also checks the Mapping and its External Identity, as for the Mapping's own credential. Identity Disablement therefore stops it until a restore; Tenant Deletion revokes none. A Mapping holds at most 20 active self-issued credentials (409 beyond). `GET .../credentials?mine=true` lists them, and the member revokes its own without another action.
- A Tenant's subject holding `credential.issue` issues credentials for service and agent subjects that no Mapping has ever named, judged from Mapping history by subject name, so changing a person's subject kind or recreating it under the same name does not open it. A person's user subject never receives a credential from someone else (400). These credentials belong to the Tenant and outlive their issuer, who is recorded in `issued_by` as `oidc:<identity id>`.
- A Deployment Administrator issues credentials for any subject, as before.

Use `Authorization: Bearer <subject token>` for scoped operations. Neither a request body nor a header can select another subject or tenant. Each protected transaction rechecks credential expiry/revocation and the current policy. A disabled subject or ancestor is denied immediately at the next boundary. Unknown, expired and revoked tokens receive HTTP 401; insufficient authority receives HTTP 403.

Cross-Node atomic manifests have a narrower durable-admission contract: every new Participant reservation checks current source and mapped authority, dedicated transaction/disclosure permissions and the underlying actions. An admitted fixed obligation survives later revocation so recovery can follow its durable COMMIT/ABORT decision. This exception does not authorize subsequent ordinary effects or reads; transaction details still require current access to all targets. Unknown racing admissions keep credential/policy revocation at HTTP 202 with `pending_transactions` until reconciled. See the [transaction authority contract](transactions.md#authority-and-acceptance) for peer mapping, disclosure, busy responses and the controls available during a visibility barrier.

Scoped workspace creation saves tenant and owner independently of editable workspace state, in the same transaction as the workspace, event and authorization decision. Resource kind is `workspace`, resource ID is its UUID, and trusted attributes are `owner` (the creating subject ID) and `workspace_id`. Environment attributes are `node_id` and `transport: "api"`. Setting `owner` or `tenant` inside workspace state never changes authority; extra creation fields are rejected.

| Operation                                               | Required action                                                                         |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Create a workspace                                      | `workspace.create`                                                                      |
| Read a workspace snapshot or include it in `/api/state` | `workspace.read`                                                                        |
| Update workspace state                                  | Both `workspace.read` and `workspace.update` (the response includes the full workspace) |
| Create a task in a workspace                            | `task.create`                                                                           |
| Append a workspace message                              | `message.create`                                                                        |
| Read or stream workspace events                         | Both `workspace.read` and `workspace.events`                                            |
| Configure the workspace's semantic index                | Both `workspace.read` and `semantic.index.manage` (#360)                                |
| Read the semantic index's cleanup state                 | Both `workspace.read` and `semantic.read`                                               |
| Stream a Run's inference progress                       | The authority of a run-scoped event of that Run (below)                                 |

For example, an allow policy with `subjects: {"ids":["alice"]}`, these actions, `resources: {"kinds":["workspace"]}` and a condition comparing resource `/owner` to literal `"alice"` grants Alice access to her workspaces in that tenant. Another tenant's Alice cannot access them. Workspace read opens the workspace. Its tasks, artifacts and messages additionally require `task.read`, `artifact.read` and `message.read` on each stored record. Artifact reads also require the producing task to be readable. Existing policies that only grant workspace actions must explicitly add these resource actions; no implicit content grant is retained.

Collection responses filter by persisted tenant/ownership and the active policy. Scoped `/api/state` includes only approved, readable Registry entries and omits peer and installation data. Run details, control responses, state collections, human-request listings and run-related events also require the run's `task.read`, `run.read`, `memory.read`, `human.read` for its human requests, and current read permission for recorded sources. The human-read gate also protects prompts and answers copied into a run journal. These filters apply to API snapshots, SSE frames and the workspace snapshots supplied to models and observation tools. Registering peers, the mesh and transaction trust remain Deployment Administrator-only; a Tenant's policy, Subject Credentials and Peer Mappings are managed under the policy actions above. Every workspace, task and Run belongs to a Tenant: the Deployment Administrator token cannot create workspaces, tasks, messages or conversations, claim or delegate tasks, or answer a Run's human requests, and a Run without an authorization record never executes (ADR 0043). A Deployment Administrator who wants to run work does so inside a Tenant, through a Mapping or a Subject Credential. The Deployment Administrator token retains privileged bootstrap, administration, reads, Run controls and recovery access.

SSE validates scope before sending HTTP 200 and reloads credential/policy authority before each event frame, including already buffered events. It holds no database lock while yielding a frame. Polling without delivery does not append repeated decision audits. Credential revocation interrupts the stream; permission revocation prevents further matching delivery and denies reconnection to the revoked workspace.

`GET /api/runs/{run}/inference/stream` is authorized exactly like a journal event carrying that Run's `data.run_id`: for a Subject, the Run's workspace must pass the stream workspace check and the Run must be visible (`task.read`, `run.read`, `memory.read`, `human.read` and current source reads); for a Deployment Administrator, the Run's workspace must not require Home retrieval. A Run whose Home is another node has no local workspace and is not streamed to Subjects, as with its journal events. The check runs before HTTP 200, before each frame under a fresh visibility lease and every five seconds while idle. Run-read or source revocation closes the stream and denies reconnection; credential revocation closes it like the event stream.

## Tenant catalog and local execution

Every Tenant has its own Registry. A definition's identity is `(tenant, id, version)`, and versions are immutable: editing a definition registers a new version. Built-in definitions (`aidash.` and `core.` identifiers) are written into every Tenant's Registry when the Tenant is created and again at startup (ADR 0033). References resolve only within the Tenant's Registry and its built-ins.

| Method | Path                                             | Required action                                                    |
| ------ | ------------------------------------------------ | ------------------------------------------------------------------ |
| POST   | `/api/registry`                                  | `registry.create` on `registry` `{kind}/{id}`                      |
| GET    | `/api/registry/versions`                         | `registry.read`: every version with its approval state and history |
| GET    | `/api/authorization/{tenant}/catalog`            | `registry.read`: the Tenant's Catalog Approvals                    |
| POST   | `/api/authorization/{tenant}/catalog`            | `catalog.approve` on `registry` `{kind}/{id}@{version}`            |
| POST   | `/api/authorization/{tenant}/catalog/approvals`  | `catalog.approve` for each version of one dependency closure       |
| POST   | `/api/marketplace/installations/{id}/activation` | `catalog.approve` on the activated revision's definition           |
| POST   | `/api/marketplace/approval-sets`                 | `catalog.approve` on every definition of the set                   |

Every path that writes a version also needs `registry.create`, besides its own action: agent configuration (`agent.configure`), workbench registration (`agent_draft.register`), Marketplace installation (`marketplace.install`) and skill import. Idempotency keys belong to `(tenant, principal, key)`; a replay decides the action again, and the same key with different input returns 409. A denial or a failure leaves no definition, approval, document, idempotency row or event.

A Catalog Approval decides that one exact version may be used in the Tenant. Approval changes use optimistic revisions and keep their history, with the deciding subject as actor. An approval is refused while any dependency (an agent's model, context summarizer and bindings, a bundle's members, a cluster's coordinator) is neither approved nor an ordinary built-in; one `/catalog/approvals` request can approve the whole closure. Withdrawing an approval is never blocked, and execution then refuses the version. Built-in tools need no approval, except Sandboxed tools, Research providers and Deciders, which each Tenant approves before use (ADR 0034).

Approving an agent version also makes the agent a subject of the Tenant's policy, `aidash://<node>/agents/<id>@<version>`, so that it can run (ADR 0062). The subject joins `members` when the policy marks it Assignable; naming other Assignable Groups in `groups` also needs `subject.group.update`. It has no delegator: its authority is its groups intersected with each Run's chain. Approving again leaves an existing subject unchanged, so it never re-enables a disabled one, and naming groups for an existing subject returns 409. `PUT /api/tenants/{tenant}/subjects/{subject}/groups` moves agent subjects between Assignable Groups like user subjects.

Approval alone does not grant a policy action; policy permission alone does not expose an unapproved entry. Discovery and execution see only approved versions satisfying `registry.read`. Scoped API and worker discovery search the local node and source-authorized peers. Worker decisions intersect every subject in the originating execution chain.

Approving a generation needs `generation.approve` and `catalog.approve`, and a Deployment Administrator cannot approve one. At publication the generated version is approved only while its approver still holds `catalog.approve`; otherwise, and on the receiving Node of a foreign generation, it is registered unapproved.

A subject can POST `{"revision":0,"agent":{"id":"research","version":"1.0.0"}}` to `/api/tasks/{id}/claim`, or delegate to the local node through `/api/tasks/{id}/delegate`. The task must be OPEN with completed dependencies. Admission checks workspace access, `agent.execute` and `task.execute`, then commits the task claim, run and execution grant together. Local delegation additionally requires `task.delegate` for the originating chain and records its delivery atomically.

The grant stores tenant, workspace, root subject, credential ID and the complete runtime agent chain; it contains no bearer secret. Every execution decision must allow both the originating subject and every agent in that chain, including each subject's policy-defined delegation ancestors. A child cannot escape a parent's denial. The chain is bounded to 32 subjects, including the root.

| Boundary                   | Required action and resource                                                                                                                       |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Every worker step          | `workspace.read` on the workspace, `task.execute` on the task, `agent.execute` on the agent, `run.read` on the run and `memory.read` on its memory |
| Load configured components | `registry.read` on the approved model/tool/skill/cluster entries                                                                                   |
| Model inference            | `model.infer` on the model, `skill.use` on each skill and `memory.read`                                                                            |
| Invoke a tool              | `tool.invoke` on the configured tool ID or `builtin:<tool name>`                                                                                   |
| Read a Skill asset         | `skill_asset_read`: `skill.use` on a configured Registry Skill, or the Agent's Skills Core capability for a direct Skill                           |
| Builtin mutations          | `task.create`, `task.delegate`, `artifact.create`, `message.create`, `memory.write` or `human.request` for the corresponding resource              |
| Complete a task            | `artifact.create` and `task.complete`                                                                                                              |
| Control or resume a run    | `workspace.read`, `run.read`, `memory.read` and `run.control`                                                                                      |

Under a `deferred@1` Exposure policy, `capability_search`, `capability_describe`, `capability_load` and `capability_unload` add no Agent flag, Core capability or resource requirement beyond the Run authority above and the `tool.invoke` check every bound Built-in passes. They disclose and change only the Run's own Binding snapshot and Exposure set. Loading a capability grants nothing: every Binding's authority is still checked at each step, and revocation fails the step whether or not the capability is loaded. `skill_asset_read` identifies its Skill by alias and decides by identity kind: a Registry Skill must be configured on the Agent and pass `skill.use`, and a direct Skill requires the Skills Core capability. An alias that names no Registry Skill on an Agent without Skill attachments or roots names no asset; it reaches the tool, which returns the recoverable `UNKNOWN_CAPABILITY`.

Runtime resource attributes include trusted workspace `owner` and `workspace_id`. Tasks add their stored `created_by` and `task_id`; artifacts add their stored `created_by`, `task_id` and content `kind`; messages expose their persisted author as `created_by` and `sender`. Registry resources add their stored version, capabilities, tags, languages and configuration. Memory uses the agent ID with its stored run's `version` and canonical agent `created_by`. JSON payloads cannot replace these authority attributes. Artifact creation is scoped to the producing task ID; task/message creation is scoped to the workspace ID. The environment identifies the node and `transport: "worker"`. These attributes come from persisted state.

Each worker boundary reloads and locks the current policy, credential, grant, workspace ownership and relevant catalog approvals. Its decision audits commit before model/tool effects start. Revocations wait for an in-flight boundary to finish, then prevent the next boundary. A denied or revoked execution pauses while retaining phase, context and pending tool cursor. Restoring permissions and resuming continues the saved operation under a fresh check. A fresh credential for the original root subject can replace an expired/revoked grant credential through resume; the agent chain remains unchanged. Authorized cancellation can clean up after revocation without new model or tool calls.

The `serve` process reserves a separate database pool for workers so API requests waiting to revoke authority cannot exhaust the connections needed to finish an effect. Embedded runners should construct their worker Federation with `Federation::for_workers()` before starting workers. Pool settings and connection initialization hooks are preserved. Deploy this version to all workers before admitting scoped runs; older workers do not enforce execution grants.

Scoped remote delegation uses durable source grants, receiver admissions and scoped Worker/Home boundaries, described below. It is the only remote delegation: the unscoped peer offer, workspace-command and control endpoints are removed (ADR 0043).

## Scoped conversations and human interaction

The dashboard uses a Google OIDC session (or an explicitly configured legacy Keycloak session) and a selected mapping or Deployment Administrator context. `/api/state` returns the authenticated access profile; the dashboard uses it to show tenant/subject identity and avoid administrator-only mesh, Marketplace and registration flows. Authorization remains enforced on the server. Invalid, expired or revoked sessions clear the dashboard's authenticated cache and return to the sign-in screen. Access-denied and reconnect messages support en-US and ja-JP.

| Operation                                     | Additional required actions                                                                                                             |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Start a conversation with an agent or cluster | `conversation.create` and `conversation.read`, workspace create/read, message/task create, task read/delegate/execute and agent execute |
| Select a cluster coordinator                  | `registry.read` and `cluster.execute` on the approved cluster version                                                                   |
| Send a message to a run                       | `run.message` and `message.create`, plus the run's read gates                                                                           |
| Answer a human request                        | `human.read` and `human.answer`, plus the run's read gates                                                                              |
| Abandon a failed, blocked or cancelled task   | `workspace.read`, `task.read` and `task.abandon`                                                                                        |

Conversation admission atomically creates workspace ownership, conversation, initial message, task, local delegation and execution grant. A late policy denial rolls back all these changes while retaining its authorization decisions. Conversation and message authors come from the authenticated subject. The returned task reflects its committed claim. Cluster approvals are rechecked at every worker boundary, even when the agent configuration does not repeat the conversation's cluster reference.

Conversation reads and events require `conversation.read`; trusted attributes include `created_by`, `target`, `target_kind`, workspace ID and owner. Human-request reads and events require `human.read`; their attributes include request `kind`, `run_id`, workspace ID and owner. A worker cannot consume a denied human response. The human-request membership of a run is rechecked during event replay so a newly created denied request cannot bypass a cached run decision.

Human answers persist `answered_by`. Repeating the same answer preserves the original actor and produces no duplicate answer event; conflicting answers return HTTP 409. Task abandonment retains the existing revision/child-state constraints and records the authenticated actor in its event. Cross-tenant interaction IDs return HTTP 403 before exposing resource state.

`POST /api/runs/{id}/message` accepts instructions for an active run. Accepted messages are included in subsequent inference requests only after the executor can read their authorized workspace records; a later `message.read` denial pauses scoped execution. The aggregate size of run messages is bounded against the selected model's request headroom (at most one quarter, capped at 16,384 estimated serialized bytes); an oversized new message returns HTTP 400 without being recorded. Oversized messages imported from old runs remain available by message ID; the worker requires the agent to read their complete records with `workspace_read` before accepting a final answer. Read coverage is durable across context compaction. Remote messages are admitted on the executor before their home-workspace copy is delivered, and a worker retries any pending copy before inference or terminal reconciliation. For a remote run, the home node reserves the task under a row lock before executor admission; task termination returns HTTP 409 until a successful inference has observed the accepted input. A rejected admission releases its reservation. Explicit cancellation or failure supersedes an unobserved correction after its home copy is delivered, and consumes its fence atomically with task termination. Reservations for corrections not yet admitted by the executor remain active and block that transition. Terminal runs retain pending copies in a durable outbox for later delivery. Binding contract 1 requires protocol 0.2 on both Nodes and the Home peer's terminal-safe delivery capability; earlier peers cannot admit new execution or corrections. Drain in-flight work before upgrading the fleet. During rolling upgrades, a database gate rejects old local replicas' run-directed writes that lack ledger admission, including unkeyed writes while a workspace has active runs. New workspace-message handlers generate keys when callers omit them. Recovered remote runs reconcile home history before processing a pending final response. When a message arrives during final inference or while its answer is pending publication, the worker discards that answer and starts a fresh inference before publishing its final response and artifact. New messages sent once final completion starts, after cancellation or another terminal transition has been queued, or after the run is terminal, return HTTP 409 instead of a successful `sent` response. Clients should create an explicit follow-up task for a correction rejected at this boundary. Retrying a previously accepted message with the same idempotency key remains safe, including keys accepted before the run-input migration.

Read coverage and inference coverage are kept outside the Context Projection, so neither pruning nor an Execution Summary can make an unread message read. Message inference coverage still counts only content present in an accepted provider request; a summary of a read is not complete-read evidence. The Summary Stage absorbs only events that already reached an accepted inference and records every message a read call targeted as a summary dependency. The dependency comes from the call, so it survives when Jev has already truncated the read result to its leading text. Before each inference, a local worker rechecks those dependencies against current `message.read` authority without recording a new read; a remote executor rechecks them through the Home's filtered record reads, the path that admitted the original reads. If any is no longer readable, the worker discards the summary, saves the original events restored from the Context Journal (`context.summary_revoked`) and pauses the Run for authority (`run.authorization_blocked`) before Jev pruning or the model can receive them.

## Resource reads and journal sources

Workspace snapshots, `/api/state`, event replay, every SSE frame, model context and observation tools filter individual tasks, artifacts and messages using the same stored-resource resolvers. Run visibility requires its task to be readable. Creation and assignment responses also enforce task read permission: unreadable new-task responses roll back their writes while retaining the denial audit, and unreadable parents/dependencies cannot be used to create a new task. A claim cannot return or execute a task that the caller cannot read.

New message events include the immutable message ID. Older events lacking IDs require read permission for every matching stored message; ambiguous identity never bypasses a denied record. Unknown event families are withheld until an explicit scoped reader exists. A completion event containing both task and artifact requires both reads.

A worker commits the IDs of records in an authorized workspace snapshot to `authorization_run_reads` before a provider or tool can copy them into a durable journal. Its created tasks, artifacts, messages and generation assignments are recorded in the same transaction as their output, including idempotent replay. API reads, controls, event replay and worker boundaries recheck those sources against current policy. Revoking a source or output hides an affected journal and pauses its execution before another model/tool effect, even if a previously authorized snapshot is already in history. Sources withheld from a new snapshot are not recorded and do not block otherwise authorized work. Dependencies between observed runs are traversed iteratively, including cycles; source membership and human-request membership are rechecked rather than assuming cached membership is complete.

Registry entries returned by agent discovery are recorded separately in `authorization_run_registry_reads` before their metadata enters an invocation or context. Journal reads and worker boundaries recheck the current tenant catalog binding and `registry.read` permission for every copied entry. The migration recovers these dependencies from existing contexts, pending responses and invocation results. Scoped artifact collections fill the visible result limit after both artifact and task authorization filtering.

The migration conservatively associates older nonempty journals with their existing workspace records because they did not record exact source membership. Those older journals may therefore need broader read grants than newly recorded snapshots. This protects retained data instead of treating missing provenance as permission to disclose it. Independently authorized output records retain their own read policies; this ledger does not claim arbitrary information-flow tracking for generated prose. Semantic-memory sources have a separate revision and read-dependency ledger.

## Dashboard administration

The **Access policies / アクセス制御** page opens a named tenant for a Deployment Administrator, and its own Tenant for a subject whose Tenant's policy grants `policy.read`; each control appears only where the matching action is held. Create or edit the full policy bundle, including subjects, groups, inherited roles, delegation and attribute conditions. The editor pins the revision that was opened, presents the complete replacement before applying it, and preserves the draft after a validation or revision conflict. A conflicting update refreshes the latest saved policy without silently rebasing or overwriting the draft.

The decision form evaluates the current saved revision. It shows allow/deny, the reason, matched policies and effective roles in English or Japanese. Simulation writes no audit; a Deployment Administrator may select **Save this decision to the audit history** to record the same scenario. These supplied scenarios do not execute the action or establish trusted resource attributes. Editing a scenario or observing a newer policy revision clears its previous result.

Credential issuance selects a subject and bounded lifetime. The bearer token appears once in a dialog, is not added to the query cache or browser storage, and disappears when dismissed, changing tenants or leaving the page. Credential metadata shows expiry and revocation. Revocation requires reviewing the selected credential and takes effect at subsequent authorization boundaries. Component approval selects an exact Registry ID/version; enabling or disabling it uses the latest observed binding revision.

Change history and decision audits paginate using durable revision/sequence cursors. Failed refreshes hide stale policy, credential, catalog and history results. Subject sessions see only the panels their Tenant's actions permit, and never the option to save a scenario to the decision log. Browser acceptance exercises inherited roles, attribute mismatches, explicit deny, simulation/audit separation, optimistic conflicts, token issuance/revocation, catalog version approval, pagination, tenant switching and English/Japanese desktop/mobile rendering against real API and PostgreSQL state.

## Policy semantics

Subjects have kind `user`, `agent`, `node` or `service`, direct roles, groups, attributes, an `enabled` flag and an optional `delegated_by` subject. Groups grant roles; roles inherit other roles. Subject selectors combine IDs, kinds, groups and effective roles with OR. Use `{"any": true}` alone to match any registered, enabled subject in the tenant. Actions and resource kinds must be nonempty sets. Resource IDs optionally narrow the scope. Names are case-sensitive; only a complete `"*"` is a wildcard in action/resource selectors.

Policies default to deny, and any matching explicit deny overrides all allows regardless of order. A resource in another tenant always denies. Delegated subjects must also pass the same action/resource evaluation for every ancestor, using that ancestor's current attributes and authority. Disabling a delegator revokes its descendants' effective authority on the next evaluation.

Conditions support `all`, `any`, `eq`, `not_eq`, array `contains` and `exists`. Operands read JSON pointers from subject/resource/environment attributes or contain a literal JSON value. Missing operands fail comparisons, including `not_eq`; explicit JSON null remains a value. Empty condition groups, unknown fields, dangling references, duplicate policy IDs, role/delegation cycles and excessive expression depth are rejected. Bundles allow at most 512 subjects, groups, roles and policies each; role/delegation chains are limited to 32, and each expression to depth 16 and 256 nodes.

Decisions include the revision, reason, matched policy IDs and the subject's effective roles. Audits retain identifiers and the decision, omitting raw subject/resource/environment attributes. Dry-run requests produce no writes.

## Transaction boundary

`Authorization::evaluate_in_transaction` holds a shared lock on the policy revision until the caller commits or rolls back. A mutation integration must execute its protected change in that same transaction; calling `evaluate`, then starting a separate mutation transaction would allow revocation to race the operation. Scoped compound operations hold the policy/credential locks before a mutation savepoint and defer their decision audit until finalization; a policy denial rolls back protected writes before committing the audit. Long-running work must recheck at each persisted execution or delivery boundary.

PostgreSQL integration tests verify API authentication, revision conflicts, decision/history persistence, dry-run behavior, revoked delegation, two-tenant workspace isolation, ownership forgery rejection, credential expiry/revocation, buffered SSE revocation and policy/credential locks held through a real workspace mutation. Local worker tests cover approved catalog isolation, disabled-agent and tool-policy denial, pending-operation recovery, child authority intersection, credential rotation, cancellation after revocation, run visibility in control/collection/stream/tool paths, durable audit before an HTTP effect and revocation with all API pool connections occupied. Pure policy tests cover inherited group roles, attribute conditions, deny precedence, tenant isolation, missing attributes and invalid policy graphs. These tests do not establish mesh-wide enforcement or completion of FR-AUTH-001.

Approved generation compaction uses resource kind `compactor` and action `compaction.invoke`, in addition to `registry.read`. The worker intersects all originating subjects and retains the live policy/credential/catalog lease through each bounded provider call. See [the compaction contract](generation.md#approved-compaction).

Generated semantic queries and background indexing use resource kind `embedding` and action `embedding.invoke`, alongside `registry.read` and current tenant catalog approval. Every generated ancestor must pin the same exact provider, remain live and supply both call and token allowance. Background jobs retain their initiating authority. See [the embedding contract](generation.md#approved-embeddings).

## Inbound peer identity mappings

### Federated Graph View authority

Graph View calls `GET /api/federation/graph/peers` on the local Node to list
directly configured Peers visible to the current viewer. A Subject needs
`federation.graph.read` on each destination Node. A Deployment Administrator must have a
currently authenticated named browser identity; the shared recovery API token
cannot read a remote graph. Topology is available to authorized Subjects.

Expansion calls `POST /api/federation/graph` with one `node_id`, a Graph mode,
an optional local `scope_workspace`, `depth: 1`, allowlisted `kinds` and `relations`, an
activity window (`hours`), a page `limit` of 2–80 and an optional opaque
`cursor`. A Deployment Administrator also selects exactly one `target_tenant` at the receiving
Node. The local Node checks the active viewer and selected Workspace before
sending the request over its configured Peer credential. It never accepts a
caller supplied Subject or Deployment Administrator identity for forwarding.

The receiving Node serves `POST /federation/v0.1/scoped/graph` only to an
authenticated Peer. For a Subject, its exact inbound Peer mapping and current
local credential, policy and delegation ancestors must allow
`federation.graph.read` on the receiving Node. Each resource then passes its
existing read gate and tenant catalog approval. The source Node's local
`registry.read` policy does not replace the receiver's decision for a B owned
resource. The mapping, Peer and authority are rechecked on every page.

For a named Deployment Administrator of another Deployment, a receiving Deployment Administrator can create or
change a Peer Graph Grant with `POST /api/authorization/{tenant}/peer-graph-grants` and list grants
with `GET` on the same path (`offset`, `limit`, maximum 200). The body contains
`source_node`, `source_identity` (the source browser identity UUID), `enabled`
and `expected_revision` (zero on creation). A Peer Graph Grant applies to one tenant and
one named source identity; it confers no Deployment Administrator authority and never turns a
subject mapping into a Deployment Administrator mapping. Disabling the grant or Peer denies the next page or refresh. Catalog
approvals and the graph display allowlist still apply under a Peer Graph Grant.

The projection returns only authorized display fields and edges backed by an
authorized record. A readable Workspace's Goal node carries its full current
Goal body; task descriptions, artifact content, message bodies, human requests,
raw Registry configuration and raw Event data are excluded. Message activity
requires `workspace.events`, `message.read` and a visible endpoint. There is
no transitive Peer traversal. Pages use a short lived cursor bound to the
source Peer, viewer, tenant, requested scope and projection generation; changes
to current authority or relevant data invalidate continuation. An oversized
Goal fails explicitly instead of returning a shortened body.

A Deployment Administrator, or a Tenant's subject holding `peer_mapping.update` (through a browser Mapping of that Tenant), can bind an authenticated source node, source tenant and source subject to an existing local subject credential using `POST /api/authorization/{tenant}/peer-mappings`. The body contains `source_node`, `source_tenant`, `source_subject`, `credential_id`, `enabled` and `expected_revision` (zero for creation). `GET` on the same path lists the tenant's mappings with `offset` (default zero) and `limit` (default 100, maximum 200). A source identity has one target tenant in the whole Deployment; it cannot be reassigned through another tenant's management endpoint, and a mapping that would collide answers 409 without naming the Tenant that holds it. Per-Tenant uniqueness waits for #241. Concurrent or stale revisions return a conflict. Disabled mappings can retain expired or revoked credentials for audit, but enabling one requires a currently usable credential and configured peer.

A mapping binds only an exportable credential: never a Mapping's own and never one a member issued for itself, since either would let someone outside the Deployment act as a person (403). A Tenant's subject enables a mapping only to a credential it could issue itself now, that is, one of a service or agent subject on which it holds `credential.issue`, so a Peer Mapping never confers more than its author could. Disabling needs only `peer_mapping.update`. Listing and history need `peer_mapping.read`. The mapping and its history record the actor: `deployment_administrator` or `oidc:<identity id>`.

`GET /api/authorization/{tenant}/peer-mapping-history?after=0&limit=100` returns committed revisions in ascending sequence order. Use the last `sequence` as the next cursor; the maximum page size is 200. Failed or conflicting updates do not create history. All mapping management and history endpoints require the Deployment Administrator token.

An authenticated peer may call `POST /federation/v0.1/scoped/discover` with `{"tenant":"source-tenant","subject":"source-subject","search":{}}`. The source node comes from the authenticated peer header, never the JSON body. Peer authentication alone grants no tenant access: an enabled exact mapping and its current local credential and subject chain are required. The request holds policy, credential, mapping and enabled-peer database leases through discovery. The local policy must allow `federation.discover` on the receiving node; results must also have local tenant catalog approval, `registry.read` and `agent.execute`. ABAC receives `environment.transport=federation`, `environment.source_node`, and resource attributes `source_node`, `source_tenant`, `source_subject`. An unusable mapped credential returns forbidden without challenging the valid peer credential.

Only credential IDs are stored in mappings; subject bearer tokens are neither returned by mapping APIs nor forwarded between nodes. This endpoint provides inbound authorized metadata discovery. Remote execution grants, remote budget accounting and their dashboard controls remain separate work; this does not establish end-to-end scoped federation.

The tenant's Access policies dashboard provides mapping creation, credential rotation, enable/disable controls and paginated mapping history in English and Japanese. Identity fields are fixed after creation; the editor submits the revision it opened so a concurrent change cannot be overwritten. Revoked and expired credentials cannot be selected for an enabled mapping. Failed reads hide stale records, and a rejected update leaves its error visible. A Tenant's subject sees these controls when its Tenant's policy grants `peer_mapping.read`, and edits them under `peer_mapping.update`.

Subject requests to `POST /api/discover` use `/scoped/discover` on each permitted peer. The source policy must allow `federation.discover` on the peer's node resource before any source tenant, subject or search query is disclosed. Disallowed peers are omitted entirely. The source retains policy, credential and peer-configuration leases through the operation; up to eight peer requests run concurrently with the existing bounded transport. Subject bearer tokens are never forwarded. An unavailable peer produces an explicit per-node error without forwarding its response body, and there is no fallback to legacy discovery.

Remote entries additionally require source-side `registry.read` and `agent.execute`. Their resource kind is `agent`, their resource ID is `{node}/agents/{id}@{version}`, and `remote_node` is a trusted resource attribute. The receiving node still applies its own mapped identity, policy and exact catalog approvals. Matching remote and local entry IDs therefore remain separate policy resources. Responses reject non-Agent entries, malformed metadata, duplicate versions and more than 1024 entries per peer; accepted entries also pass the original search filter. Remote execution admission still requires durable cross-node execution grants; discovery does not grant execution authority.

Worker discovery durably records remote metadata, its node-qualified version and its digest before saving a tool result or model context. Every subsequent worker boundary and scoped journal read verifies those records against the current source policy and the receiving node through `POST /federation/v0.1/scoped/registry/verify`. Verification uses the current reader's mapped identity, requires receiver `registry.read` and node `federation.discover`, and returns only a boolean for the supplied exact references and hashes. It requires neither subject bearer forwarding nor remote metadata retransmission. Missing mappings, revoked credentials, changed definitions, unavailable peers and withdrawn catalog approvals deny the read. Results are cached only within the current access boundary; new requests and worker boundaries recheck them.

Authored tasks, artifacts and messages retain explicit producer membership separately from consumed inputs. Their readers, event filters and semantic ingestion also check the producer's source dependencies. Cycles through a run's own output terminate without skipping checks on its external sources. Migration recovers older authored artifact/message membership from recorded reads and authorship; new output membership commits with the resource itself. This protects derived results as well as raw journals after source revocation. Integration tests exercise a real two-node discovery, persistence across fresh worker connections, remote approval withdrawal before another model call, restoration and completion, then credential revocation hiding journals, derived artifacts and message events.

### Receiver execution preflight

`POST /federation/v0.1/scoped/execution/inspect` accepts `{"tenant":"source-tenant","subject":"source-subject","agent":{"id":"research","version":"1.0.0"},"requirements":{}}`. It uses the same authenticated peer mapping and retained authority leases as discovery. The mapped identity and the receiver's registered Agent subject must both allow `federation.execute` on the receiving node. Their intersected policies and the tenant catalog must allow the exact Agent (`agent.execute`), model (`model.infer`), tools (`tool.invoke`), skills (`skill.use`) and optional cluster (`cluster.execute`), plus `registry.read` for each definition. The requested Agent must satisfy the task's search requirements.

The response contains the node ID, authorized Agent metadata, and deterministic exact-version/digest references for its execution definitions. Authorized dependency metadata accompanies the references so the source can evaluate its own attribute policies. Credential environment references remain references; secret values are not resolved or returned. This is a current-policy preflight, not a transferable execution grant: it creates no run, reserves no authority and expires logically as soon as policy or catalog state changes. The source grant and receiver admission bind these definitions before explicit activation; both nodes recheck their current authority at subsequent durable boundaries.

### Durable remote grant preparation

A subject can POST `{"id":"<uuid>","node_id":"aidash://receiver","agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300}` to `/api/tasks/{id}/remote-grants`. The UUID is an idempotency key; retries preserve the original expiry and cannot replace the originating credential or execution definitions. Lifetimes are 1–3600 seconds. The source must register the remote Agent as an Agent subject. The originating task's inherited subject chain and that remote Agent must all allow workspace/task access, delegation, execution, `federation.execute`, and the qualified remote definitions' read/use actions. Definition resource IDs are `{node}/{kind}s/{id}@{version}`, with trusted `remote_node`, `version` and `digest` attributes alongside the receiver's metadata, including capabilities, tags and configuration for ABAC. The receiver independently checks its mapped identity and local executor.

The durable record pins the source credential, subject chain, task revision, receiver definitions and a digest identifying the receiver's mapped credential and subjects. It survives new database connections. Changed task revisions, definition substitution, credential rebinding, expiry and revocation invalidate it. POST `/api/tasks/{id}/remote-grants/{grant}/revoke` revokes it idempotently; the current originating subject must still have task read/delegation permission. Preparation and revocation produce task-filtered events without including credentials or execution configuration.

Only the recorded destination peer can POST `{"grant_id":"<uuid>"}` to `/federation/v0.1/scoped/execution/grants/verify`. It returns a boolean after checking the persisted source authority and re-inspecting the receiver's current authority and definitions. An unavailable receiver fails closed. A grant ID is not a bearer credential. Preparation and verification alone do not enqueue work; the separate activation path consumes a receiver admission and creates the scoped run.

### Receiver admission records

An authenticated source peer POSTs `{"grant_id":"<uuid>"}` to `/federation/v0.1/scoped/execution/admissions`. The receiver fetches `/scoped/execution/grants/describe` from that source; only the grant's destination can obtain the source-authorized task and definition snapshot. The receiver then takes a fresh mapped policy/credential/catalog lease, compares the frozen definition and identity digests, and checks foreign workspace/task access for both its mapped subject and executor. Resource IDs are `{source}/workspaces/{uuid}` and `{source}/tasks/{uuid}`. Trusted `source_workspace_id`, `source_task_id` and `source_task_revision` attributes also apply to model/tool/skill/cluster decisions. Authority RPCs distinguish forbidden authority from transport failure without copying remote response bodies.

The receiver persists one admission per source task and source grant. Concurrent retries return the same admission ID; a different grant or identity cannot silently replace it. Fresh connections reload the same binding. POST `/scoped/execution/admissions/{id}/verify` rechecks the source and receiver authority; outages, revocation, task/definition changes and credential rebinding fail closed. No source task event is published into an unscoped receiver workspace.

Admission records alone do not create or enqueue a run. `POST /api/tasks/{id}/remote-grants/{grant}/activate` verifies the source grant, creates or reuses the bound receiver admission and activates its scoped run. The receiver executes the pinned Agent version and uses `/scoped/execution/commands` for authorized Home reads and mutations. Worker and Home boundaries recheck the source grant and receiver authority; changes or outages fail closed. No unscoped worker boundary exists. The [scoped remote execution suite](../server/src/apps/identity/tests/scoped_remote_execution.rs) covers two separate databases and HTTP nodes, completion at Home, replay, revocation, outage and restart cases; #38 remains open until its full acceptance matrix is verified.

### Scoped source snapshots

The grant destination can POST `{"grant_id":"<uuid>"}` to `/federation/v0.1/scoped/execution/grants/snapshot`. The source retains its live credential/policy lease while checking both nodes and filtering the grant's workspace snapshot. The caller cannot select a different workspace. Before returning data, the source durably records the visible task, artifact, message and event-resource dependencies under the grant, separately from local run IDs. Subsequent snapshot, description and verification requests recheck those dependencies, including originating runs' transitive registry, semantic and remote reads. Revocation or deletion of an observed resource fails closed rather than silently removing it from a context already delivered to the receiver. This source-read endpoint alone neither activates a receiver Worker nor authorizes Home mutations; those require the admitted scoped run and command path above.
